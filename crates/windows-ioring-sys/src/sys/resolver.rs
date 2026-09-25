// Copyright (c) Mike Grier
//! A seeded resolver over the permitted kernel response space (M26.3).
//!
//! This is the thing [RESPONSE-SPACE.md](../RESPONSE-SPACE.md) was written
//! for. It implements [`Responses`] by answering the submission-path calls
//! itself, choosing a point in that space from a seed.
//!
//! # It asserts nothing about Windows
//!
//! The distinction [D-52](../DESIGN-NOTES.md#d-52) turns on, restated here
//! because it is what makes this defensible where a mock was refused twice: a
//! mock encodes a belief about what the platform *does*, and can be wrong
//! about it. This encodes a specification of what this crate will *tolerate*,
//! and the assertions made against it are about this crate. There is no belief
//! here to be wrong -- only a space that could be too narrow, which is a
//! reviewable defect rather than a hidden one.
//!
//! So every freedom below cites the clause that permits it, and every
//! restriction cites the clause that requires it. A clause no code cites is
//! visibly unimplemented; a behaviour citing no clause is a resolver inventing
//! a platform.
//!
//! # Permissions are configurable; constraints are not
//!
//! [`ResolverConfig`] has a switch per `RS-P-n` and none for any `RS-C-n`, and
//! that asymmetry is deliberate rather than incidental. A permission is a
//! freedom a test may wish to narrow in order to isolate another -- a test
//! about ordering does not want arbitrary operation failures on top. A
//! constraint is what makes this crate's code reviewable at all, so a knob
//! that relaxed one would let a test quietly assert against a platform that
//! cannot exist. Constraints are therefore only reachable by editing this
//! file, which is what the sabotage manifest does to show they are
//! load-bearing.
//!
//! # What drives resolution
//!
//! A real kernel completes pending work on its own. This has no background, so
//! its only clock is being consulted:
//!
//! - **A submit resolves.** Each still-unresolved operation independently
//!   either completes now or pends (`RS-P-1`).
//! - **A pop only prevents starvation.** It advances every operation's
//!   deferral count and posts anything that has run out, but it flips no
//!   coins. Without that split a consumer polling [`crate::IoRing::try_pop`]
//!   would see its first poll resolve the work, and "the operation pended" --
//!   the thing `RS-P-1` exists to let a test observe -- would be unobservable.
//!
//! The deferral bound is what makes `RS-C-1` finite. It is a property of this
//! resolver, not of the space: the space deliberately carries no rates, and a
//! bound on how long a resolver may defer is a mechanism for satisfying a
//! constraint rather than a claim about how often Windows pends.
//!
//! # Two ordering requirements, and what it costs to install one
//!
//! **Install before the ring exists, and drop the guard after the ring.** The
//! resolver answers for operations the kernel never saw. If the guard drops
//! first, the ring's own rundown asks a real ring to wait for completions it
//! has no pending operation for, which `SubmitIoRing` answers `E_INVALIDARG`.
//! [`Resolver::scoped`] arranges both by construction and is the shape to
//! reach for.
//!
//! **A ring under a resolver is answered per thread**, as
//! [`super::installed`] explains. Moving such a ring to a thread with nothing
//! installed hands its operations straight to a kernel that never received
//! them.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::rc::Rc;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLUSH_MODE, IORING_BUFFER_INFO, IORING_BUFFER_REF, IORING_CQE, IORING_HANDLE_REF,
    IOSQE_FLAGS_DRAIN_PRECEDING_OPS,
};
use windows_sys::Win32::System::Threading::SetEvent;
use windows_sys::core::HRESULT;

use super::installed::{Installed, Responses};

/// Environment override for the resolver seed (decimal, or `0x` hex).
///
/// **A third axis, kept separate on purpose.**
/// [generated_sequences.rs](../../tests/generated_sequences.rs) already
/// carries two -- its own sequence seed and the guard allocator's -- and
/// [D-41](../DESIGN-NOTES.md#d-41)'s discipline is that one number replays one
/// thing. Folding the resolution choices into either of those would produce a
/// replay that reproduced part of a run and not the rest, which is worse than
/// no replay at all because it looks like one.
pub const SEED_VAR: &str = "WINDOWS_IORING_RESOLVER_SEED";

/// How many consultations an operation may pend across before this resolver
/// posts it regardless.
///
/// This is how `RS-C-1` ("every submitted operation eventually completes")
/// becomes finite rather than merely eventual. Four is chosen so that a
/// pending operation survives a few polls -- enough for a test to observe that
/// it pended -- without a rundown loop taking long enough to look like a hang.
const MAX_DEFERRALS: u32 = 4;

/// How many consecutive submits `RS-P-7` may fail before this resolver lets
/// one through.
///
/// Same role as [`MAX_DEFERRALS`], for the other way an operation can be made
/// to wait: a resolver free to fail every submit forever would never complete
/// anything, which `RS-C-1` forbids.
const MAX_CONSECUTIVE_SUBMIT_FAILURES: u32 = 2;

/// `S_OK`.
const S_OK: HRESULT = 0;
/// `S_FALSE`, which is how `PopIoRingCompletion` reports an empty queue.
const S_FALSE: HRESULT = 1;
/// `IORING_E_WAIT_TIMEOUT`, which `SubmitIoRing` documents as "all operations
/// were submitted without error and the subsequent wait timed out".
///
/// Spelled by derivation for the reason `ring.rs` records at its own copy: the
/// name is a macro over `HRESULT_FROM_WIN32(ERROR_TIMEOUT)` rather than a
/// `FACILITY_IORING` code, so the bindings emit no constant to import.
const IORING_E_WAIT_TIMEOUT: HRESULT =
    (0x8007_0000_u32 | windows_sys::Win32::Foundation::ERROR_TIMEOUT) as HRESULT;
/// `HRESULT_FROM_WIN32(ERROR_NOT_ENOUGH_MEMORY)`, the failure this resolver
/// reports for a submit it declines under `RS-P-7`.
///
/// Deliberately **not** `IORING_E_WAIT_TIMEOUT`: that code carries a positive
/// guarantee that every entry was submitted, so using it here would make the
/// resolver claim the work went in while holding it back.
const SUBMIT_FAILED: HRESULT =
    (0x8007_0000_u32 | windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) as HRESULT;

/// Which permissions this resolver exercises.
///
/// Every field names its clause. All default to `true` -- the widest point in
/// the space -- so that narrowing is a visible, deliberate act in a test
/// rather than something a resolver quietly failed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolverConfig {
    /// `RS-P-1`: an operation may complete inside `SubmitIoRing`, or pend.
    ///
    /// With this off every operation completes inside the submit that carried
    /// it, which is the narrowest reading of the clause and the one a test
    /// isolating some *other* freedom usually wants.
    pub may_pend: bool,
    /// `RS-P-2`: completion order is unconstrained.
    ///
    /// With this off completions are posted in submission order. That is not
    /// a promise Windows makes -- [D-47](../DESIGN-NOTES.md#d-47) measured it
    /// broken -- so a test turning this off is narrowing to isolate, never
    /// asserting FIFO.
    pub may_reorder: bool,
    /// `RS-P-3`: an operation may fail individually.
    pub may_fail_operations: bool,
    /// `RS-P-4`: a wait may expire, including when completions are available.
    pub may_expire_waits: bool,
    /// `RS-P-5`: a wait may return successfully with nothing poppable.
    pub may_wake_empty: bool,
    /// `RS-P-6`: a completion posted while the queue was already non-empty
    /// need produce no signal.
    ///
    /// With this off every post signals. That is *wider* than the platform --
    /// [D-19](../DESIGN-NOTES.md#d-19) measured the event as edge triggered --
    /// so it exists to show the edge behaviour is load-bearing, not as a
    /// configuration any consumer should rely on.
    pub edge_triggered_signal: bool,
    /// `RS-P-7`: a submit may fail, leaving built operations queued for a
    /// later one.
    pub may_fail_submits: bool,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            may_pend: true,
            may_reorder: true,
            may_fail_operations: true,
            may_expire_waits: true,
            may_wake_empty: true,
            edge_triggered_signal: true,
            may_fail_submits: true,
        }
    }
}

impl ResolverConfig {
    /// Every permission off: operations complete inside their submit, in
    /// order, successfully, and no wait misbehaves.
    ///
    /// The narrowest point in the space, and therefore the *weakest* test. It
    /// exists so a test can turn on exactly the one freedom it is about, and
    /// so a reader can tell that choice from an accident.
    #[must_use]
    pub const fn narrowest() -> Self {
        Self {
            may_pend: false,
            may_reorder: false,
            may_fail_operations: false,
            may_expire_waits: false,
            may_wake_empty: false,
            edge_triggered_signal: true,
            may_fail_submits: false,
        }
    }
}

/// What a resolution actually did, readable while the resolver is installed.
///
/// A test that narrows the config to isolate one freedom needs to know the
/// freedom was *exercised*; otherwise a green run means only that the seed
/// never took that branch. These counters are how a test asserts against the
/// run rather than against the configuration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResolverStats {
    /// Operations built (any `Build*` call that returned success).
    pub built: usize,
    /// Operations moved from built to submitted.
    pub submitted: usize,
    /// Completions posted to the completion queue.
    pub posted: usize,
    /// Completions handed to a `PopIoRingCompletion`.
    pub popped: usize,
    /// Posts that carried a failure result (`RS-P-3` exercised).
    pub failed_operations: usize,
    /// Submits declined, leaving built operations queued (`RS-P-7`).
    pub failed_submits: usize,
    /// Waits answered as expired (`RS-P-4`).
    pub expired_waits: usize,
    /// Waits answered successfully with nothing poppable (`RS-P-5`).
    pub empty_wakes: usize,
    /// Event signals raised (`RS-P-6`; zero when no event is attached).
    pub signals: usize,
    /// Posts that took something other than the oldest unresolved operation
    /// (`RS-P-2` exercised -- reordering actually happened, as distinct from
    /// having been permitted).
    pub reorderings: usize,
    /// Times an operation was left unresolved by a consultation (`RS-P-1`
    /// exercised -- something actually pended).
    pub deferrals: usize,
    /// Times a drain-flagged operation was held back because something queued
    /// before it was still unresolved (`RS-C-4` actually bit, as distinct from
    /// having been vacuously satisfied).
    pub barrier_holds: usize,
}

/// A live view of [`ResolverStats`] for an installed resolver.
///
/// `Rc` rather than `Arc` because a responder is thread-local
/// ([`super::installed`]), so sharing it across threads is already
/// meaningless.
#[derive(Debug, Clone)]
pub struct ResolverWatch(Rc<RefCell<ResolverStats>>);

impl ResolverWatch {
    /// The statistics as of now.
    #[must_use]
    pub fn stats(&self) -> ResolverStats {
        *self.0.borrow()
    }
}

/// One operation the resolver is answering for.
#[derive(Debug, Clone, Copy)]
struct Op {
    /// `RS-C-2`: echoed into the completion exactly as supplied.
    user_data: usize,
    /// Build order, ring-wide. `RS-C-4` is stated over this.
    seq: u64,
    /// Carries `IOSQE_FLAGS_DRAIN_PRECEDING_OPS`.
    barrier: bool,
    /// Consultations survived without being posted.
    deferrals: u32,
}

/// A resolver over [RESPONSE-SPACE.md](../RESPONSE-SPACE.md).
///
/// See the module documentation for what drives resolution and for the two
/// ordering requirements installing one imposes.
pub struct Resolver {
    seed: u64,
    state: u64,
    config: ResolverConfig,
    /// Built, not yet submitted. `RS-C-3` is exactly the rule that nothing
    /// here is eligible to complete.
    staged: Vec<Op>,
    /// Submitted, not yet posted. Held in build order, which is what lets
    /// `RS-C-4` be decided by position.
    pool: Vec<Op>,
    /// Posted, not yet popped. The completion queue.
    posted: VecDeque<(usize, HRESULT)>,
    next_seq: u64,
    consecutive_submit_failures: u32,
    /// The ring's completion event, if one has been attached. Borrowed, never
    /// owned: the ring closes it, and this resolver must not outlive the ring.
    event: *mut c_void,
    stats: Rc<RefCell<ResolverStats>>,
}

impl std::fmt::Debug for Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolver")
            .field("seed", &format_args!("0x{:016X}", self.seed))
            .field("config", &self.config)
            .field("staged", &self.staged.len())
            .field("pool", &self.pool.len())
            .field("posted", &self.posted.len())
            .field("stats", &*self.stats.borrow())
            .finish()
    }
}

impl Resolver {
    /// A resolver at the widest point in the space, from an explicit seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::with_config(seed, ResolverConfig::default())
    }

    /// A resolver exercising exactly the permissions `config` allows.
    #[must_use]
    pub fn with_config(seed: u64, config: ResolverConfig) -> Self {
        Self {
            seed,
            // `| 1` so a zero seed is not a fixed point of the mixer.
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            config,
            staged: Vec::new(),
            pool: Vec::new(),
            posted: VecDeque::new(),
            next_seq: 0,
            consecutive_submit_failures: 0,
            event: std::ptr::null_mut(),
            stats: Rc::new(RefCell::new(ResolverStats::default())),
        }
    }

    /// The seed this resolver is replaying.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// A live view of what this resolution has done so far.
    #[must_use]
    pub fn watch(&self) -> ResolverWatch {
        ResolverWatch(Rc::clone(&self.stats))
    }

    /// The seed for this run: pinned from [`SEED_VAR`] when set, otherwise
    /// derived from the clock so successive runs explore different points.
    ///
    /// # Panics
    ///
    /// If [`SEED_VAR`] is set to something that is neither decimal nor `0x`
    /// hex. A seed that silently fell back to the clock would make the
    /// variable look like it worked while the run it was pinning drifted.
    #[must_use]
    pub fn seed_from_env() -> u64 {
        match std::env::var(SEED_VAR) {
            Ok(text) => {
                let trimmed = text.trim();
                trimmed
                    .strip_prefix("0x")
                    .or_else(|| trimmed.strip_prefix("0X"))
                    .map_or_else(
                        || trimmed.parse::<u64>().ok(),
                        |hex| u64::from_str_radix(hex, 16).ok(),
                    )
                    .unwrap_or_else(|| {
                        panic!("{SEED_VAR} is set to {text:?}, which is neither decimal nor 0x hex")
                    })
            }
            Err(_) => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x243F_6A88_85A3_08D3, |elapsed| elapsed.as_nanos() as u64),
        }
    }

    /// The command that replays this resolver's choices.
    ///
    /// Print it from any test that installs one. It names **only this axis**:
    /// a test that also draws on another seeded source announces that source
    /// too, because a partial replay is the failure `SEED_VAR` documents.
    #[must_use]
    pub fn replay_hint(&self) -> String {
        format!("$env:{SEED_VAR}='0x{:016X}'", self.seed)
    }

    /// Install this resolver, run `body`, then uninstall it.
    ///
    /// This is the shape to reach for, because it makes the module's first
    /// ordering requirement structural: a ring created inside `body` is also
    /// dropped inside it, so its rundown is still answered by the resolver
    /// that owns its operations.
    ///
    /// # Panics
    ///
    /// If a responder is already installed on this thread.
    pub fn scoped<R>(self, body: impl FnOnce(&ResolverWatch) -> R) -> R {
        let watch = self.watch();
        let guard: Installed = super::install(Box::new(self));
        let outcome = body(&watch);
        drop(guard);
        outcome
    }

    /// SplitMix64, matching the generator this crate already uses
    /// ([D-41](../DESIGN-NOTES.md#d-41)): one number replays a whole run.
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// True with probability `percent`.
    ///
    /// The weighting is a property of this resolver, never of the space --
    /// [RESPONSE-SPACE.md](../RESPONSE-SPACE.md) carries no rates, deliberately,
    /// because a space with observed probabilities in it is a recording.
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn record(&self, f: impl FnOnce(&mut ResolverStats)) {
        f(&mut self.stats.borrow_mut());
    }

    /// Record a built operation and return the success every `Build*` call
    /// returns when it queues an SQE.
    fn build(&mut self, user_data: usize, flags: i32) -> HRESULT {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.staged.push(Op {
            user_data,
            seq,
            barrier: flags & IOSQE_FLAGS_DRAIN_PRECEDING_OPS != 0,
            deferrals: 0,
        });
        self.record(|s| s.built += 1);
        S_OK
    }

    /// Post one completion, choosing which under `RS-P-2` and `RS-C-4`.
    ///
    /// Returns false only when there is nothing to post.
    fn post_one(&mut self) -> bool {
        if self.pool.is_empty() {
            return false;
        }

        // `RS-C-4`: no operation queued before a drain-flagged operation may
        // complete after it. `pool` is in build order, so the set queued
        // before `pool[i]` is exactly `pool[..i]` -- a barrier is therefore
        // eligible only when it is the oldest unresolved operation.
        //
        // That reduction is the whole of this constraint's implementation, and
        // it is load-bearing rather than descriptive: it holds only while the
        // pool stays sorted by build order. An edit that sorted or reshuffled
        // the pool would relax `RS-C-4` to nothing while every line below kept
        // reading as though it still applied, so the invariant is asserted
        // rather than asserted-in-a-comment.
        debug_assert!(
            self.pool.windows(2).all(|pair| pair[0].seq < pair[1].seq),
            "the pool must stay in build order or RS-C-4's positional test is meaningless"
        );
        //
        // Note which way this resolves against `RS-P-2`: ordering is
        // "unconstrained" as a permission, and a constraint overrides a
        // permission. The hold-back half is *not* constrained -- D-24 claimed
        // it and D-47 withdrew the claim -- so an operation queued after a
        // barrier is eligible at any time, including before the barrier.
        let eligible: Vec<usize> = (0..self.pool.len())
            .filter(|&i| i == 0 || !self.pool[i].barrier)
            .collect();
        let held = self.pool.len() - eligible.len();
        if held > 0 {
            self.record(|s| s.barrier_holds += 1);
        }

        let pick = if self.config.may_reorder {
            // `RS-P-2`: uniform over everything eligible. Repeated to
            // exhaustion this is a uniform shuffle of each barrier-delimited
            // segment, which is the widest order the constraint leaves.
            let draw = self.next() % eligible.len() as u64;
            eligible[usize::try_from(draw).expect("a modulus of a usize length fits a usize")]
        } else {
            eligible[0]
        };
        if pick != 0 {
            self.record(|s| s.reorderings += 1);
        }

        let op = self.pool.remove(pick);

        // `RS-P-3`: any single operation may fail while its neighbours
        // succeed. The *set* of codes is a property of this resolver; the
        // space enumerates none and says a consumer must not depend on the
        // set being small, so this draws across the whole Win32 facility
        // rather than from a handful of plausible ones.
        let result = if self.config.may_fail_operations && self.chance(20) {
            self.record(|s| s.failed_operations += 1);
            let code = self.next() & 0xFFFF;
            (0x8007_0000_u32 | u32::try_from(code).expect("masked to 16 bits")) as HRESULT
        } else {
            S_OK
        };

        let was_empty = self.posted.is_empty();
        self.posted.push_back((op.user_data, result));
        self.record(|s| s.posted += 1);

        // `RS-P-6`: the completion event fires on the empty-to-non-empty
        // edge, so a completion arriving behind another produces no signal of
        // its own (D-19, measured). With `edge_triggered_signal` off this
        // signals every post, which is wider than the platform and exists to
        // show the edge is load-bearing.
        if !self.event.is_null() && (was_empty || !self.config.edge_triggered_signal) {
            self.record(|s| s.signals += 1);
            // SAFETY: `event` was handed to `set_completion_event` by the ring
            // that owns it, and this resolver is required to be uninstalled
            // only after that ring is gone.
            unsafe { SetEvent(self.event) };
        }
        true
    }

    /// Advance resolution by one consultation.
    ///
    /// `flip` distinguishes the two clocks the module documents: a submit
    /// flips coins, a pop only rescues operations that have run out of
    /// deferrals.
    fn tick(&mut self, flip: bool) {
        if self.pool.is_empty() {
            return;
        }

        let mut want = 0_usize;
        for i in 0..self.pool.len() {
            let forced = self.pool[i].deferrals >= MAX_DEFERRALS;
            // `RS-P-1`: each operation independently completes now or pends.
            // A forced post is `RS-C-1` overriding that permission, which is
            // the same precedence `RS-C-4` takes over `RS-P-2`.
            if forced || (flip && (!self.config.may_pend || self.chance(50))) {
                want += 1;
            }
        }

        let deferred = self.pool.len() - want.min(self.pool.len());
        if deferred > 0 {
            self.record(|s| s.deferrals += deferred);
        }
        for op in &mut self.pool {
            op.deferrals += 1;
        }

        for _ in 0..want {
            if !self.post_one() {
                break;
            }
        }
    }
}

impl Responses for Resolver {
    unsafe fn submit(
        &mut self,
        _ring: *mut c_void,
        wait_operations: u32,
        _milliseconds: u32,
        submitted: *mut u32,
    ) -> HRESULT {
        // `RS-P-7`: a failed submit leaves already-built operations queued for
        // a later submit -- there is no rewind once `Build*` returns (D-5).
        // Bounded, because a resolver that failed every submit forever would
        // never complete anything and `RS-C-1` forbids that.
        if self.config.may_fail_submits
            && !self.staged.is_empty()
            && self.consecutive_submit_failures < MAX_CONSECUTIVE_SUBMIT_FAILURES
            && self.chance(15)
        {
            self.consecutive_submit_failures += 1;
            self.record(|s| s.failed_submits += 1);
            // SAFETY: the caller passes a valid out-pointer, as the Win32 call
            // requires.
            unsafe { submitted.write(0) };
            return SUBMIT_FAILED;
        }
        self.consecutive_submit_failures = 0;

        // `RS-C-3`: nothing completes before it is submitted. That rule is
        // this move and nothing else -- only `pool` is ever eligible to post.
        let moved = self.staged.len();
        self.pool.append(&mut self.staged);
        self.record(|s| s.submitted += moved);
        // SAFETY: as above.
        unsafe {
            submitted.write(u32::try_from(moved).unwrap_or(u32::MAX));
        }

        self.tick(true);

        if wait_operations == 0 {
            return S_OK;
        }

        // `RS-P-4`: a wait may expire, and may do so even when completions are
        // available and on the very first call. This is not a ring failure --
        // M21.6 fixed a defect in this crate that read it as one.
        if self.config.may_expire_waits && self.chance(25) {
            self.record(|s| s.expired_waits += 1);
            return IORING_E_WAIT_TIMEOUT;
        }

        // `RS-P-5`: a wait returning success promises nothing about
        // poppability. Reached whenever the queue is empty here -- either
        // because resolution deferred everything, or because this clause
        // chose to wake with work still pending.
        if self.posted.is_empty() {
            self.record(|s| s.empty_wakes += 1);
        }
        S_OK
    }

    unsafe fn pop(&mut self, _ring: *mut c_void, cqe: *mut IORING_CQE) -> HRESULT {
        if self.posted.is_empty() {
            // Starvation rescue only: no coins. See the module's note on the
            // two clocks -- flipping here would make `RS-P-1`'s pending case
            // unobservable to a polling consumer.
            self.tick(false);
        }

        let Some((user_data, result)) = self.posted.pop_front() else {
            return S_FALSE;
        };
        self.record(|s| s.popped += 1);
        // SAFETY: the caller passes a valid out-pointer, as the Win32 call
        // requires.
        unsafe {
            cqe.write(IORING_CQE {
                // `RS-C-2`: a completion identifies the operation that
                // produced it, by carrying back exactly the value supplied
                // when it was built.
                UserData: user_data,
                ResultCode: result,
                Information: 0,
            });
        }
        S_OK
    }

    unsafe fn build_read(
        &mut self,
        _ring: *mut c_void,
        _file: IORING_HANDLE_REF,
        _buffer: IORING_BUFFER_REF,
        _bytes: u32,
        _offset: u64,
        user_data: usize,
        flags: i32,
    ) -> HRESULT {
        self.build(user_data, flags)
    }

    unsafe fn build_write(
        &mut self,
        _ring: *mut c_void,
        _file: IORING_HANDLE_REF,
        _buffer: IORING_BUFFER_REF,
        _bytes: u32,
        _offset: u64,
        _caching: i32,
        user_data: usize,
        flags: i32,
    ) -> HRESULT {
        self.build(user_data, flags)
    }

    unsafe fn build_flush(
        &mut self,
        _ring: *mut c_void,
        _file: IORING_HANDLE_REF,
        _mode: FILE_FLUSH_MODE,
        user_data: usize,
        flags: i32,
    ) -> HRESULT {
        self.build(user_data, flags)
    }

    unsafe fn build_cancel(
        &mut self,
        _ring: *mut c_void,
        _file: IORING_HANDLE_REF,
        _cancel_user_data: usize,
        user_data: usize,
    ) -> HRESULT {
        // A cancel carries no SQE flags of its own in this crate's surface, so
        // it is never a barrier. It is still an operation and still completes
        // exactly once under `RS-C-1`.
        self.build(user_data, 0)
    }

    unsafe fn build_register_files(
        &mut self,
        _ring: *mut c_void,
        _count: u32,
        _handles: *const *mut c_void,
        user_data: usize,
    ) -> HRESULT {
        self.build(user_data, 0)
    }

    unsafe fn build_register_buffers(
        &mut self,
        _ring: *mut c_void,
        _count: u32,
        _buffers: *const IORING_BUFFER_INFO,
        user_data: usize,
    ) -> HRESULT {
        self.build(user_data, 0)
    }

    unsafe fn set_completion_event(&mut self, _ring: *mut c_void, event: *mut c_void) -> HRESULT {
        // Kept, not forwarded. The real ring would signal for operations it
        // never received; this resolver signals for the ones it is answering
        // for, on the edge `RS-P-6` describes.
        self.event = event;
        S_OK
    }
}

#[cfg(test)]
mod tests;
