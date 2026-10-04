// Copyright (c) 2026 Mike Grier
//! The owned `IoRing` handle (M1.2), and the op capability set (M1.4).

use std::collections::HashMap;
use std::mem::ManuallyDrop;

use crate::token::OperationId;
use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Storage::FileSystem::{
    CloseIoRing, CreateIoRing, GetIoRingInfo, IORING_BUFFER_INFO, IORING_CQE,
    IORING_CREATE_ADVISORY_FLAGS_NONE, IORING_CREATE_FLAGS, IORING_CREATE_REQUIRED_FLAGS_NONE,
    IORING_INFO, IORING_OP_CANCEL, IORING_OP_CODE, IORING_OP_FLUSH, IORING_OP_NOP, IORING_OP_READ,
    IORING_OP_REGISTER_BUFFERS, IORING_OP_REGISTER_FILES, IORING_OP_WRITE, IsIoRingOpSupported,
};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

use crate::accounting::Accounting;
use crate::capability::{RingVersion, capabilities};
use crate::error::check;

pub(crate) use crate::accounting::RingId;

/// One `IoRing` operation.
///
/// `#[non_exhaustive]`: the kernel's op table has grown before (M1.4, D-7)
/// and will again. A consumer must not be able to write an exhaustive
/// `match` that a new variant would break. [`IoRing::supports_raw`] reaches
/// an op this enum does not yet name.
///
/// Naming an op here is not the same as offering a way to push it: every
/// variant except [`Op::Nop`] gates one or more [`crate::Batch`] methods
/// (`Read` gates the four read pushes, `Write` the four write pushes,
/// `Flush` and `Cancel` two each, and the two registration ops one each),
/// while `Nop` gates none and is reachable only through
/// [`IoRing::push_raw`]. See [`IoRing::supports`] (M10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Op {
    /// `IORING_OP_NOP`.
    Nop,
    /// `IORING_OP_READ`.
    Read,
    /// `IORING_OP_WRITE`.
    Write,
    /// `IORING_OP_FLUSH`.
    Flush,
    /// `IORING_OP_REGISTER_FILES`.
    RegisterFiles,
    /// `IORING_OP_REGISTER_BUFFERS`.
    RegisterBuffers,
    /// `IORING_OP_CANCEL`.
    Cancel,
}

impl Op {
    /// Every op this crate names, in a fixed order used to index the cached
    /// capability set.
    const ALL: [Op; 7] = [
        Op::Nop,
        Op::Read,
        Op::Write,
        Op::Flush,
        Op::RegisterFiles,
        Op::RegisterBuffers,
        Op::Cancel,
    ];

    /// The raw `IORING_OP_CODE` value.
    #[must_use]
    pub fn code(self) -> IORING_OP_CODE {
        match self {
            Op::Nop => IORING_OP_NOP,
            Op::Read => IORING_OP_READ,
            Op::Write => IORING_OP_WRITE,
            Op::Flush => IORING_OP_FLUSH,
            Op::RegisterFiles => IORING_OP_REGISTER_FILES,
            Op::RegisterBuffers => IORING_OP_REGISTER_BUFFERS,
            Op::Cancel => IORING_OP_CANCEL,
        }
    }
}

/// Which ops a ring supports, probed once at construction.
///
/// A `u8` bitmask indexed by position in [`Op::ALL`] rather than a `HashSet`:
/// there are exactly seven possible members, known at compile time, so a
/// heap-allocating set would cost more than it buys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct OpSupport(u8);

impl OpSupport {
    /// # Safety
    ///
    /// `handle` must be a live `HIORING`.
    unsafe fn probe(handle: *mut c_void) -> Self {
        let mut mask = 0_u8;
        for (index, op) in Op::ALL.iter().enumerate() {
            // SAFETY: forwarded from the caller.
            let supported = unsafe { IsIoRingOpSupported(handle, op.code()) };
            if supported != 0 {
                mask |= 1 << index;
            }
        }
        Self(mask)
    }

    fn contains(self, op: Op) -> bool {
        let index = Op::ALL
            .iter()
            .position(|&candidate| candidate == op)
            .expect("Op::ALL is exhaustive");
        self.0 & (1 << index) != 0
    }
}

/// What [`IoRing::info`] reports back from `GetIoRingInfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RingInfo {
    /// The version this ring was actually created at.
    pub version: RingVersion,
    /// The ring's submission queue size.
    pub submission_queue_size: u32,
    /// The ring's completion queue size.
    pub completion_queue_size: u32,
}

/// A failure to report in place of a completion's real result (M16.3).
///
/// Three spellings of the same thing, chosen so a call site reads as what it
/// is testing rather than as a hexadecimal constant.
///
/// Available only under the `fault-injection` feature; see
/// [`Completion::with_injected_failure`].
#[cfg(any(test, feature = "fault-injection"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectedFailure {
    /// One of the ring's own `IORING_E_*` conditions.
    Ring(crate::RingCondition),
    /// A Win32 error code, wrapped the way the kernel wraps one -- so
    /// `(hresult as u32) & 0xFFFF` recovers it, which is how this crate's
    /// tests read a real one back.
    Win32(u32),
    /// A raw `HRESULT`, for a failure neither of the above names.
    Hresult(windows_sys::core::HRESULT),
}

#[cfg(any(test, feature = "fault-injection"))]
impl InjectedFailure {
    /// The `HRESULT` this failure puts in the completion's result code.
    ///
    /// # Panics
    ///
    /// If the result would not actually be a failure. Only
    /// [`InjectedFailure::Hresult`] can express that -- the other two spellings
    /// always produce a failing code -- and it is rejected rather than
    /// accepted, so that
    /// [`Completion::with_injected_failure`]'s "failure only, never success"
    /// guarantee is true by construction rather than by convention. A seam
    /// that could turn a real failure into an apparent success would let a
    /// test conceal the very defects it exists to find.
    fn as_hresult(self) -> windows_sys::core::HRESULT {
        let code = match self {
            Self::Ring(condition) => condition.code(),
            // `HRESULT_FROM_WIN32`: severity 1, facility 7 (`FACILITY_WIN32`),
            // and the low 16 bits of the code.
            //
            // The `|` here is provably equivalent to `^`, and a mutation run
            // will report that mutant surviving: `0x8007_0000`'s low sixteen
            // bits are zero and `code & 0xFFFF`'s high sixteen bits are zero,
            // so the two operands never share a set bit and every bitwise
            // combinator that agrees on disjoint inputs agrees here too. `|`
            // is kept because it reads as "these are separate fields" where
            // `^` would read as an arithmetic accident; no test can tell them
            // apart, so none is written.
            Self::Win32(code) => (0x8007_0000_u32 | (code & 0xFFFF)).cast_signed(),
            Self::Hresult(code) => code,
        };
        assert!(
            code < 0,
            "an injected failure must be a failing HRESULT, but {code:#010x} is not: \
             this seam injects failure only, never success"
        );
        code
    }
}

/// What the crate itself is holding for an in-flight operation.
///
/// Concrete rather than generic, and [D-73](../DESIGN-NOTES.md#d-73) explains
/// why that is sound: `FileTarget` is sealed to `SharedFile` and
/// `RegisteredFile`, so this set is closed and crate-owned. A caller never
/// names it. Unsealing that trait would break the arrangement -- the
/// alternatives are type erasure, which [D-4](../DESIGN-NOTES.md#d-4) forbids,
/// or a third generic parameter on every consumer.
#[derive(Default)]
pub(crate) struct Held {
    /// Keeps the file valid for the operation's life.
    ///
    /// Never read, and `allow` rather than `expect` says so deliberately:
    /// this field exists **for its `Drop`**, not for its value. Holding it
    /// until the pop that completes the operation is the whole job, and
    /// reading it would serve nothing. An `expect` here would be a promise
    /// that some later change makes it read, which is not the intent.
    #[allow(
        dead_code,
        reason = "held so the file outlives the operation; dropped at reclaim"
    )]
    pub(crate) guard: Option<FileGuard>,
    /// Keeps a registered buffer's use counted while the kernel has it.
    ///
    /// Held for its `Drop`, as `guard` above.
    #[allow(dead_code, reason = "held so the registration outlives the operation")]
    pub(crate) registration: Option<crate::batch::RegisteredUse>,
}

/// The closed set of file guards, per `D-73`.
///
/// Public only because [`crate::FileTarget`]'s `Guard` bound names it, and a
/// bound may not be more private than the trait carrying it. It is opaque on
/// purpose: a caller cannot construct one, and the sealed trait means nobody
/// outside this crate implements the thing that produces one.
pub enum FileGuard {
    Shared(crate::batch::SharedFile),
    Registered(crate::batch::RegisteredFile),
}

impl From<crate::batch::SharedFile> for FileGuard {
    fn from(guard: crate::batch::SharedFile) -> Self {
        Self::Shared(guard)
    }
}

impl From<crate::batch::RegisteredFile> for FileGuard {
    fn from(guard: crate::batch::RegisteredFile) -> Self {
        Self::Registered(guard)
    }
}

/// A popped completion and whatever the ring was holding for it.
///
/// Named rather than left as a nested tuple in the signature: the outer
/// `Option` is "was there a completion", and the inner one is "was this ring
/// holding anything for it", and those are different questions that read
/// badly stacked.
pub type HeldCompletion<T, X> = (Completion, Option<(Option<T>, X)>);

/// One in-flight operation's entry in the ring's inventory.
pub(crate) struct Entry<T, X> {
    /// What the caller handed over. `None` for a push that carries nothing to
    /// give back -- the `_raw` flush and cancel entry points, whose shape is
    /// `M28.5`'s to settle.
    pub(crate) payload: Option<T>,
    /// The caller's per-operation sidecar. Two thirds of the census sites keep
    /// one, which is why it is a parameter rather than a convenience.
    pub(crate) extra: X,
    /// The crate's own half, which the caller never sees.
    #[expect(
        dead_code,
        reason = "read when the guarded pushes migrate in M28.4.1; see M28.3+M28.4 in CHECKLIST.md"
    )]
    pub(crate) held: Held,
}
/// One popped completion (M3.7): the operation's identity, from
/// `IORING_CQE::UserData`, and its result.
#[derive(Clone, Copy, Debug)]
pub struct Completion {
    user_data: usize,
    result_code: windows_sys::core::HRESULT,
    information: usize,
    ring_id: RingId,
}

impl Completion {
    /// The `UserData` identity this completion reports.
    ///
    /// **Not a way to reach what the operation was holding.** The pop that
    /// produced this completion already returned that, and it is the only
    /// call that can -- see [`IoRing::try_pop`]. This is the integer for
    /// correlation and for naming a cancel target, which is the same thing
    /// [`crate::OperationId`] is and for the same reason.
    ///
    /// It used to say "match it against a held `Token`", and that instruction
    /// was [D-55](../DESIGN-NOTES.md#d-55)'s evidence that the crate handed a
    /// caller two halves and connected them with nothing.
    #[must_use]
    pub fn user_data(&self) -> usize {
        self.user_data
    }

    /// The identity of the ring that popped this completion (PR #20 review
    /// response): an entry or registration only ever matches a
    /// `Completion` whose `ring_id` is also its own, so a `UserData` value
    /// that happens to coincide across two different rings (every ring's
    /// own counter starts at the same value) can never be confused for a
    /// match.
    #[must_use]
    pub(crate) fn ring_id(&self) -> RingId {
        self.ring_id
    }

    /// This op's result: the transferred byte count (read/write) or other
    /// op-specific value in `IORING_CQE::Information`, once `ResultCode`
    /// says success.
    ///
    /// # The count may be short, and the remainder is yours
    ///
    /// A successful read or write may report **fewer bytes than were
    /// requested**, including zero (`RS-P-8` in
    /// [RESPONSE-SPACE.md](../RESPONSE-SPACE.md)). `WriteFile` documents this
    /// for non-blocking byte-mode pipes; sockets report a short send when the
    /// transmit buffer is full, and reads are short at end of file. This crate
    /// takes a handle and does not constrain what kind it is, so a consumer
    /// must compare this count against the length it submitted rather than
    /// assume they are equal.
    ///
    /// Nothing here reissues the remainder. Whether to submit another
    /// operation for it, how many times, and when to give up are the caller's
    /// (D-67), and a consumer that loops must tolerate a completion that makes
    /// no progress.
    ///
    /// A consumer that has narrowed its *own* handle type can rely on more --
    /// for an ordinary file on a local volume a successful completion is
    /// expected to carry the full length, and a full volume is an error rather
    /// than a short success. That is a guarantee such a consumer earns by
    /// constraining the handle, and it belongs in its contract rather than
    /// being assumed from this one.
    ///
    /// # Errors
    ///
    /// Returns the wrapped [`crate::IoRingError`] if `ResultCode` is a
    /// failure -- for example `ERROR_NOT_FOUND` when a cancel target was not
    /// actually outstanding.
    pub fn result(&self) -> io::Result<usize> {
        check(self.result_code)?;
        Ok(self.information)
    }

    /// Report `failure` from [`Completion::result`] instead of what this
    /// operation actually returned (M16.3).
    ///
    /// # What this is for
    ///
    /// Failure paths that are otherwise unreachable in a test. Most of what a
    /// consumer must handle -- a write that fails partway, a flush the device
    /// rejects, a registration the kernel refuses -- cannot be provoked on
    /// demand from a healthy machine, so that code is typically written once
    /// and never executed again. This crate shipped a defect of exactly that
    /// shape: a claim path that returned early on a failed write and leaked
    /// its registered-buffer slot permanently, on a branch no test had ever
    /// taken.
    ///
    /// # Why this is sound, where fabricating a completion would not be
    ///
    /// The pop's safety argument is that a [`Completion`] for some `UserData`
    /// **existing at all** proves the kernel has finished with that operation,
    /// and therefore that handing its buffer back is sound.
    ///
    /// This method consumes a completion the ring genuinely popped and returns
    /// one carrying the same `UserData` and the same ring identity. The
    /// operation really did finish; the only thing that changes is what
    /// [`Completion::result`] says about it. So the argument above is
    /// untouched, and claiming against the result is exactly as sound as
    /// claiming against the original.
    ///
    /// `Completion::synthetic` is the opposite, and is why it is not
    /// reachable from here. A fabricated completion can name an operation that
    /// is **still in flight**, and claiming a token against one hands a buffer
    /// back to the caller while the kernel is still writing through it -- the
    /// precise use-after-free this crate exists to prevent. That is a
    /// test-only, crate-only tool and stays one.
    ///
    /// # What it cannot do
    ///
    /// Only failure can be injected, never success. Turning a real failure
    /// into an apparent success would let a test conceal a genuine defect,
    /// which is the wrong affordance for a tool whose whole purpose is
    /// exercising error handling. That is enforced rather than merely stated:
    /// a non-failing `HRESULT` panics.
    ///
    /// # Panics
    ///
    /// If `failure` does not resolve to a failing `HRESULT`. Only
    /// [`InjectedFailure::Hresult`] can express that at all.
    ///
    /// # Prefer a real failure when one is reachable
    ///
    /// A genuine kernel error -- writing through a read-only handle, cancelling
    /// something that is not outstanding -- tests the real path *and* confirms
    /// the crate's own error translation. Reach for this only where no such
    /// route exists.
    ///
    /// # The one place injection is *not* inert: registration completions
    ///
    /// The soundness argument above is about the pop, where a failed
    /// completion changes nothing the caller does with memory: the buffer
    /// comes back either way. **A registration claim is different.**
    /// [`crate::PendingBufferRegistration::claim_if`] treats a failed
    /// completion as proof the kernel did *not* retain the addresses, and so
    /// **drops the buffers**. Inject a failure there and it frees memory the
    /// kernel genuinely does hold registered -- leaving the ring with a
    /// registration pointing at freed pages.
    ///
    /// That is inert only while nothing uses it. A test doing this must not
    /// push any registered-buffer operation on that ring afterwards, and
    /// should tear the ring down immediately.
    ///
    /// This is a limitation of *what a failed registration means*, not
    /// something the seam can check: a `Completion` does not carry which
    /// operation produced it, so this cannot be refused at the call. Prefer a
    /// genuine failure for registration paths wherever one can be provoked.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // ONE pop. The completion being injected into has already been popped,
    /// // and that same pop is what retired the inventory entry and handed the
    /// // payload back -- there is no second pop that could return it again.
    /// let (completion, payload) = ring
    ///     .try_pop()
    ///     .expect("try_pop")
    ///     .expect("a completion is ready");
    ///
    /// // The payload comes back whatever the result says: the operation did
    /// // complete, and a failed completion is still the proof that frees the
    /// // buffer.
    /// let completion = completion.with_injected_failure(
    ///     InjectedFailure::Win32(ERROR_ACCESS_DENIED),
    /// );
    /// assert!(completion.result().is_err());
    /// ```
    #[cfg(any(test, feature = "fault-injection"))]
    #[must_use]
    pub fn with_injected_failure(self, failure: InjectedFailure) -> Self {
        Self {
            result_code: failure.as_hresult(),
            // Zeroed deliberately: a failed operation transferred nothing, and
            // leaving a success's byte count behind would model a state the
            // kernel never produces.
            information: 0,
            ..self
        }
    }

    /// Report a **successful** transfer of `transferred` bytes instead of what
    /// this operation actually reported (M26.11).
    ///
    /// # What this is for, and why the failure seam cannot do it
    ///
    /// [`Completion::with_injected_failure`] models an operation that failed.
    /// `RS-P-8` describes something different and stranger: an operation that
    /// **succeeded** while moving fewer bytes than were asked for. Windows
    /// documents that for non-blocking byte-mode pipes and it happens on
    /// sockets, but an ordinary file on a local volume does not do it -- so a
    /// consumer's handling of a short count is written once against the
    /// documentation and then never executed again, which is exactly the class
    /// of path the failure seam was introduced for.
    ///
    /// A consumer that *narrows* its handle type may legitimately require
    /// complete transfers. This seam is how such a consumer tests that its
    /// requirement is enforced rather than merely stated -- `epoch_log`'s
    /// appender uses it for precisely that.
    ///
    /// # Why this one is inert
    ///
    /// The result code is left successful and only the byte count moves, so
    /// the pop behaves exactly as it would for the real completion: the buffer
    /// comes back, and no path keys memory ownership off the transferred
    /// count. That makes this seam free of the
    /// registration hazard documented on
    /// [`Completion::with_injected_failure`], which arises only because a
    /// *failed* registration is taken as proof the kernel retained nothing.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // A handle whose successful writes are complete is a requirement this
    /// // consumer states; here we hand it one that violates the requirement.
    /// let completion = completion.with_injected_transfer(RECORD_STRIDE - 1);
    /// assert_eq!(completion.result().expect("still a success"), RECORD_STRIDE - 1);
    /// ```
    #[cfg(any(test, feature = "fault-injection"))]
    #[must_use]
    pub fn with_injected_transfer(self, transferred: usize) -> Self {
        Self {
            // Deliberately *not* touched: a short transfer under `RS-P-8` is a
            // success, and turning it into a failure would model the one thing
            // this seam exists to distinguish it from.
            information: transferred,
            ..self
        }
    }

    /// Build a `Completion` without popping a real one, for tests that
    /// exercise the pop without real I/O.
    ///
    /// Not available outside `#[cfg(test)]`: production code has no
    /// legitimate reason to fabricate a completion, since the whole safety
    /// argument for returning a payload depends on every `Completion` in
    /// existence tracing back to a real `IORING_CQE` `IoRing::try_pop`
    /// observed.
    #[cfg(test)]
    pub(crate) fn synthetic(
        user_data: usize,
        result_code: windows_sys::core::HRESULT,
        ring_id: RingId,
    ) -> Self {
        Self {
            user_data,
            result_code,
            information: 0,
            ring_id,
        }
    }
}

/// How `S_FALSE` reads as a raw `HRESULT`: `PopIoRingCompletion`'s documented
/// "the completion queue is empty" result. Not a failure (`FAILED(hr)` is
/// false for it), so [`check`](crate::error::check) alone cannot distinguish
/// it from `S_OK`.
const S_FALSE: windows_sys::core::HRESULT = 1;

/// How long each rundown poll blocks before rechecking `outstanding`, rather
/// than one unbounded wait -- the same discipline
/// `windows-overlapped-io-sys`'s own rundown uses (its DESIGN-NOTES: "Rundown
/// waits are bounded and rechecked, not unbounded").
const RUN_DOWN_POLL_MS: u32 = 50;

/// `IORING_E_WAIT_TIMEOUT`: `SubmitIoRing` submitted every entry successfully
/// and the subsequent wait then timed out.
///
/// **This is the documented code, not an observed one.** `SubmitIoRing`'s
/// reference page gives it its own row and states the consequence that matters
/// here: *"All operations were submitted without error and the subsequent wait
/// timed out."* Its Remarks then draw the line this crate depends on -- *"If
/// this function returns an error other than IORING_E_WAIT_TIMEOUT, then all
/// entries remain in the submission queue."* So this value is the difference
/// between "the work went in" and "the work is still queued", which is why it
/// is classified rather than passed to [`check`](crate::error::check).
///
/// **Spelled by derivation because the bindings do not carry the name.**
/// `IORING_E_WAIT_TIMEOUT` is a macro over `HRESULT_FROM_WIN32(ERROR_TIMEOUT)`
/// rather than a `FACILITY_IORING` code -- that facility defines only
/// `0x8046_0001` through `0x8046_0008`, none of them a timeout -- so
/// `windows-sys` emits no constant for it and there is nothing to import. The
/// derivation below is therefore the name, and is written out so the next
/// reader does not re-derive it from a run. The `0x8007_0000` is
/// `HRESULT_FROM_WIN32`'s severity-plus-`FACILITY_WIN32` prefix, which that
/// macro ors onto any code of `0xFFFF` or less.
const IORING_E_WAIT_TIMEOUT: windows_sys::core::HRESULT =
    (0x8007_0000_u32 | windows_sys::Win32::Foundation::ERROR_TIMEOUT) as windows_sys::core::HRESULT;

/// The largest `timeout_ms` a [`CompletionWait`] is ever handed: one below
/// `u32::MAX`, because `u32::MAX` is Win32's `INFINITE`.
///
/// A waiter built on `WaitForSingleObject` or `WaitForMultipleObjects` -- both
/// of which [`CompletionWait`] explicitly invites -- reads that value as "no
/// timeout", so saturating onto it would convert a long but finite bound into
/// an unbounded block, with the pop loop unable to re-check its own deadline
/// until the wait returned.
const MAX_WAIT_MS: u32 = u32::MAX - 1;

/// Whether a `SubmitIoRing` result means every entry was submitted.
///
/// `S_OK` and `IORING_E_WAIT_TIMEOUT` both do, per that call's documented
/// return values; every other error means the opposite, and its Remarks say
/// so in as many words -- the entries remain in the submission queue.
///
/// Exposed as one predicate because three callers need the same answer and a
/// fourth got it wrong for a year: [`IoRing::pop_within`] reported a timeout
/// as a failure until `M21.6`, and [`crate::Batch::submit_and_wait`] still did
/// until `M26.8`, because that fix swept two of the three sites.
pub(crate) fn every_entry_was_submitted(hr: windows_sys::core::HRESULT) -> bool {
    hr == IORING_E_WAIT_TIMEOUT || check(hr).is_ok()
}

/// Classify the result of a `SubmitIoRing` call made **only** to wait.
///
/// An expired wait is the ordinary outcome of asking to block for a bounded
/// time, not a failure -- so it is `Ok`, and the caller re-checks whatever it
/// was waiting for. Everything else is a real error.
///
/// This exists because getting it wrong is silent and was: `check(hr)`
/// straight through turns every timed-out wait into an `Err`, which made
/// [`IoRing::pop_within`] report a timeout as a failure rather than as the
/// `Ok(None)` it documents, and made [`IoRing::run_down`] treat any operation
/// slower than [`RUN_DOWN_POLL_MS`] as fatal. One classification, two callers,
/// so they cannot disagree again.
fn wait_outcome(hr: windows_sys::core::HRESULT) -> io::Result<()> {
    if hr == IORING_E_WAIT_TIMEOUT {
        return Ok(());
    }
    check(hr)
}

/// An owned `IoRing`, closed with `CloseIoRing` on drop.
///
/// Not `Clone`: cloning would give two owners of the same native ring, and
/// `CloseIoRing` would run twice. Not `Sync`: building a submission is not
/// thread-safe (D-5 in `DESIGN-NOTES.md`), so sharing `&IoRing` across
/// threads is deliberately not offered -- a consumer wanting concurrent
/// access chooses a delivery architecture (M4 / M6+) rather than relying on
/// this type to serialize for them.
// `Debug` is hand-written rather than derived: `IORING_BUFFER_INFO` does not
// implement it, and the array's contents (raw addresses and lengths) are not
// useful to print anyway -- its length is.
pub struct IoRing<T = (), X = ()> {
    handle: *mut c_void,
    version: RingVersion,
    supported_ops: OpSupport,
    /// The half of this ring that never touches the kernel: identity,
    /// operation identities, and the counts (M24.2). Split out so those rules
    /// can be tested without opening a ring -- see [`crate::accounting`].
    accounting: Accounting,
    /// The `IORING_BUFFER_INFO` array handed to `BuildIoRingRegisterBuffers`,
    /// kept alive because the kernel reads it when the registration op
    /// *runs*, not when the `Build*` call returns (D-32, measured).
    ///
    /// Held by the ring rather than by the `Batch` that built it: a failed
    /// `SubmitIoRing` leaves the SQE queued as ring state (D-5), so a later,
    /// unrelated submit can be what finally runs it -- after that batch is
    /// long gone. A ring accepts at most one buffer registration, so this is
    /// one small allocation per ring, and it is released only by
    /// `CloseIoRing`.
    registered_buffer_infos: Vec<IORING_BUFFER_INFO>,
    /// The completion event this ring created and attached, once
    /// [`IoRing::completion_event`] has been called (M11.1, D-20).
    ///
    /// The ring owns it and hands callers duplicates, which is what makes
    /// the returned handle safe to hold for any length of time: closing a
    /// duplicate cannot leave the ring signalling a closed handle, and the
    /// kernel still signals the surviving duplicates.
    ///
    /// Dropped *after* `CloseIoRing` runs, because a manual `Drop` impl's
    /// body runs before its fields are dropped -- so the ring is closed, and
    /// can no longer signal, before the event it referenced goes away.
    completion_event: Option<OwnedHandle>,
    /// What each in-flight operation is holding on the caller's behalf
    /// (`D-71`, `M28.3`).
    ///
    /// The ring owns this rather than handing a caller a `Token` to keep
    /// beside its own map, because a consumer that never holds one cannot
    /// lose one ([D-55](../DESIGN-NOTES.md#d-55)). An entry goes in when a
    /// push queues and comes out at the pop that observes its completion --
    /// which is why there is no call turning an identity back into memory the
    /// kernel may still be using.
    ///
    /// `T` defaults to `()` so a consumer holding nothing never names it.
    inventory: ManuallyDrop<HashMap<usize, Entry<T, X>>>,
}

impl<T, X> std::fmt::Debug for IoRing<T, X> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IoRing")
            .field("handle", &self.handle)
            .field("version", &self.version)
            .field("supported_ops", &self.supported_ops)
            // One field rather than five, because the ledger derives `Debug`
            // and prints its own. Keeping the five spelled out here would be a
            // second copy of the field list, drifting the moment either side
            // gains a field.
            .field("accounting", &self.accounting)
            .field(
                "registered_buffer_infos",
                &self.registered_buffer_infos.len(),
            )
            .field("completion_event", &self.completion_event)
            .finish()
    }
}

// SAFETY: HIORING is a Windows kernel object handle. Windows handles are not
// tied to the thread that created them and may be closed from any thread, so
// moving ownership of one to another thread is sound. This does not imply
// `Sync`: submitting to the ring is not thread-safe (D-5), so only `Send` is
// implemented.
unsafe impl<T: Send, X: Send> Send for IoRing<T, X> {}

/// Constructors for a ring that holds nothing on a caller's behalf.
///
/// These sit on `IoRing<()>` rather than on the generic impl for a reason that
/// is about inference, not about capability: a defaulted type parameter
/// applies in *type* position, so `IoRing::new(..)` on a generic impl would be
/// ambiguous and every existing call site would have to write `IoRing::<()>`.
/// A ring that holds payloads is built with
/// [`IoRing::with_inventory`](IoRing::with_inventory) instead.
impl IoRing<()> {
    /// Create a ring, negotiating the version as `min(RingVersion::HIGHEST_KNOWN,
    /// capabilities()?.max_version)` (D-6).
    ///
    /// # Errors
    ///
    /// Returns any error from `QueryIoRingCapabilities` or `CreateIoRing`.
    pub fn new(submission_queue_size: u32, completion_queue_size: u32) -> io::Result<Self> {
        let caps = capabilities()?;
        let version = RingVersion::HIGHEST_KNOWN.min(caps.max_version);
        Self::with_version(version, submission_queue_size, completion_queue_size)
    }

    /// Create a ring at exactly `version`, without negotiation.
    ///
    /// # Errors
    ///
    /// Returns [`IoRingError`](crate::IoRingError) wrapping
    /// `IORING_E_VERSION_NOT_SUPPORTED` if `version` exceeds what the system
    /// supports, or any other error from `CreateIoRing`.
    pub fn with_version(
        version: RingVersion,
        submission_queue_size: u32,
        completion_queue_size: u32,
    ) -> io::Result<Self> {
        Self::with_version_and_inventory(version, submission_queue_size, completion_queue_size)
    }
}

impl<T, X> IoRing<T, X> {
    /// Create a ring at exactly `version`, whose inventory holds `T`.
    ///
    /// # Errors
    ///
    /// As [`IoRing::with_version`].
    pub fn with_version_and_inventory(
        version: RingVersion,
        submission_queue_size: u32,
        completion_queue_size: u32,
    ) -> io::Result<Self> {
        let flags = IORING_CREATE_FLAGS {
            Required: IORING_CREATE_REQUIRED_FLAGS_NONE,
            Advisory: IORING_CREATE_ADVISORY_FLAGS_NONE,
        };
        let mut handle: *mut c_void = std::ptr::null_mut();
        // SAFETY: `handle` is a valid out-pointer; `flags` is a documented,
        // all-`_NONE` value.
        let hr = unsafe {
            CreateIoRing(
                version.raw(),
                flags,
                submission_queue_size,
                completion_queue_size,
                &raw mut handle,
            )
        };
        check(hr)?;
        // SAFETY: `handle` was just created successfully above and is not
        // shared with anything else yet.
        let supported_ops = unsafe { OpSupport::probe(handle) };
        Ok(Self {
            handle,
            version,
            supported_ops,
            accounting: Accounting::new(),
            registered_buffer_infos: Vec::new(),
            completion_event: None,
            inventory: ManuallyDrop::new(HashMap::new()),
        })
    }
}

/// A ring that holds `T` on the caller's behalf for each in-flight operation.
impl<T, X> IoRing<T, X> {
    /// Whether this ring can be believed when it says nothing is outstanding.
    ///
    /// Asks the ledger, which tracks outstanding operations by **identity**:
    /// [`Accounting::record_completion`] retires only a `user_data` it minted
    /// and has not yet retired, so a CQE this ring never minted -- a duplicate,
    /// or one carrying foreign user data -- retires nothing. That holds for
    /// every push alike. An earlier revision kept a saturating counter and
    /// consulted the inventory beside it as the witness the counter had been
    /// corrupted, but only owned pushes stow, so for `push_raw` and the raw
    /// flush and cancel forms the inventory was empty whether or not anything
    /// was outstanding, and the false quiesce survived on exactly the seam
    /// whose caller-managed memory it endangers.
    ///
    /// Every owned entry's identity is still in flight until the pop that
    /// retires it also reclaims it, so the inventory is a subset of what this
    /// checks; the `debug_assert` records that rather than relying on it.
    ///
    /// Every quiescence decision asks here rather than re-deriving it. The
    /// first version of this check was written inline in `Drop` and nowhere
    /// else, so [`IoRing::run_down_within`] went on reporting the false quiesce
    /// as `Ok(true)` and `pop_within_raw_with` as "nothing can arrive": one
    /// rule at three sites, corrected at one.
    fn is_quiescent(&self) -> bool {
        let quiescent = self.accounting.is_quiescent();
        // Not asserted during an unwind: this runs inside `Drop`'s rundown,
        // and a second panic there aborts the process instead of failing the
        // test that is already unwinding.
        debug_assert!(
            !quiescent || self.inventory.is_empty() || std::thread::panicking(),
            "an owned entry outlived its identity's retirement"
        );
        quiescent
    }

    /// Pop every currently available completion, recording each -- without
    /// interpreting it, since rundown only needs to know a completion
    /// happened, not what it was.
    ///
    /// Whatever each entry held is dropped here rather than handed back,
    /// because there is nobody to hand it to: rundown is the one path that
    /// pops without a caller waiting for the result. Dropping is correct
    /// rather than merely convenient -- the completion is the proof the kernel
    /// has finished, which is exactly what makes freeing safe.
    fn drain_for_rundown(&mut self) -> io::Result<()> {
        while let Some((_completion, _held)) = self.try_pop()? {}
        Ok(())
    }

    /// A completion was popped for an identity that is not in flight on this
    /// ring ([D-79](../DESIGN-NOTES.md#d-79)).
    ///
    /// It was never minted here, has already completed, or belongs to a
    /// reservation released because its `Build*` call failed. Every queued SQE
    /// produces exactly one completion carrying the identity it was built
    /// with (M10.2), so each of those is a defect -- in a [`IoRing::push_raw`]
    /// closure that built with some other `user_data`, in a `kernel-seam`
    /// responder that broke `RS-C-2`, or in the kernel.
    ///
    /// Traced always, so the event survives in a trace dump whatever happens
    /// next, and then a panic -- **except during an unwind**. Rundown runs in
    /// `Drop`, so this can be reached while the thread is already panicking,
    /// and a second panic there would abort the process; the panic already in
    /// flight is left to carry the failure. The completion has retired
    /// nothing either way, so quiescence is unaffected ([D-78](../DESIGN-NOTES.md#d-78)).
    #[cold]
    fn unminted_completion(&self, user_data: usize) {
        #[cfg(feature = "threadpool")]
        windows_threadpool_sys::trace_record!(
            "ring",
            "unminted-completion",
            user_data,
            self.accounting.outstanding()
        );
        if !std::thread::panicking() {
            panic!(
                "IoRing popped a completion for user_data {user_data:#x}, which is not in flight \
                 on this ring: it was never minted here, has already completed, or its build \
                 failed. Every queued SQE completes exactly once with the identity it was built \
                 with, so this is a defect in a push_raw closure, a kernel-seam responder, or \
                 the kernel (D-79)"
            );
        }
    }

    /// Pop a completion without touching the inventory.
    ///
    /// The shared body of every pop. It is deliberately private: a pop that
    /// leaves the entry behind strands whatever the ring was holding, which is
    /// a defect rather than a mode -- `drain_for_rundown` had exactly that bug
    /// before `M28.4.1d.1`. Both public forms retire the entry; they differ
    /// only in whether the caller is handed what it contained.
    fn pop_raw(&mut self) -> io::Result<Option<Completion>> {
        let mut cqe = IORING_CQE {
            UserData: 0,
            ResultCode: 0,
            Information: 0,
        };
        // SAFETY: `self.handle` is a live ring; valid out-pointer.
        let hr = unsafe { crate::sys::pop(self.handle, &raw mut cqe) };
        if hr == S_FALSE {
            return Ok(None);
        }
        check(hr)?;
        if !self.record_completion(cqe.UserData) {
            self.unminted_completion(cqe.UserData);
        }
        Ok(Some(Completion {
            user_data: cqe.UserData,
            result_code: cqe.ResultCode,
            information: cqe.Information,
            ring_id: self.accounting.ring_id(),
        }))
    }

    /// Pop one completion, blocking in the ring's own wait until one is
    /// available or `timeout` elapses (M21.2).
    ///
    /// This is the join between [`IoRing::try_pop`], whose `None` means
    /// "empty at this instant", and [`crate::Batch::submit_and_wait`], whose
    /// return deliberately promises nothing about poppability because its
    /// timeout may have expired. Neither one alone answers "give me the
    /// completion I just caused", and before this existed every caller wrote
    /// that loop again -- four different ways across five sites, two of them
    /// unbounded spins.
    ///
    /// `Ok(None)` means the timeout elapsed, or that **nothing can arrive**:
    /// with no operation outstanding and an empty queue, no completion is
    /// possible, so this returns immediately rather than sleeping out the
    /// full `timeout`. That early return is what turns "you forgot to submit"
    /// from a timeout into an instant answer.
    ///
    /// A zero `timeout` is exactly one [`IoRing::try_pop`], which is the
    /// honest reading of "wait no time at all".
    ///
    /// Uses [`SubmitWait`], which blocks inside `SubmitIoRing` with no new
    /// entries queued. It does **not** touch the ring's completion event, so
    /// it cannot disturb a caller who owns that event under
    /// [D-21](../DESIGN-NOTES.md#d-21). Use
    /// [`IoRing::pop_within_with`] to supply a different wait.
    ///
    /// # Errors
    ///
    /// Any error from `SubmitIoRing` or `PopIoRingCompletion`.
    ///
    /// # Panics
    ///
    /// As [`IoRing::try_pop`], on a completion for an identity not in flight.
    pub fn pop_within(&mut self, timeout: Duration) -> io::Result<Option<HeldCompletion<T, X>>> {
        self.pop_within_with(&mut SubmitWait, timeout)
    }

    /// [`IoRing::pop_within`] with a caller-chosen wait.
    ///
    /// The crate cannot pick the wait for you, and that is a contract rather
    /// than a shrug: the completion event is auto-reset with exactly one
    /// waiter per ring ([D-21](../DESIGN-NOTES.md#d-21)), so a wait this crate
    /// chose could consume an edge the caller's own loop was entitled to. The
    /// owner of the ring is the only party who can discharge that obligation,
    /// which is why the choice is a parameter.
    ///
    /// # Errors
    ///
    /// Any error from the wait or from `PopIoRingCompletion`.
    ///
    /// # Panics
    ///
    /// As [`IoRing::try_pop`], on a completion for an identity not in flight.
    pub fn pop_within_with<W: CompletionWait + ?Sized>(
        &mut self,
        wait: &mut W,
        timeout: Duration,
    ) -> io::Result<Option<HeldCompletion<T, X>>> {
        let Some(completion) = self.pop_within_raw_with(wait, timeout)? else {
            return Ok(None);
        };
        let held = self
            .reclaim(completion.user_data())
            .map(|entry| (entry.payload, entry.extra));
        Ok(Some((completion, held)))
    }

    /// [`IoRing::pop_within_with`] without touching the inventory.
    ///
    /// The counterpart to [`IoRing::pop_raw`], and private for the same
    /// reason: it is the shared body, not a mode any caller should choose.
    fn pop_within_raw_with<W: CompletionWait + ?Sized>(
        &mut self,
        wait: &mut W,
        timeout: Duration,
    ) -> io::Result<Option<Completion>> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if let Some(completion) = self.pop_raw()? {
                return Ok(Some(completion));
            }
            // Checked *after* the pop, never before: `record_completion` runs
            // during `try_pop`, so reading it first would race the very
            // completion being drained.
            //
            // `is_quiescent`, which is keyed by identity, so a foreign or
            // duplicate completion cannot cut the wait short by claiming
            // nothing can arrive.
            if self.is_quiescent() {
                return Ok(None);
            }
            // `checked_add` rather than `+`: `Instant + Duration` panics on
            // overflow, so a caller passing `Duration::MAX` -- a reasonable
            // spelling of "no deadline" -- would take down the process. A
            // deadline the clock cannot represent is treated as one that
            // never arrives, which is what the caller asked for.
            let remaining = match deadline {
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                None => Duration::MAX,
            };
            if remaining.is_zero() {
                return Ok(None);
            }
            // Clamped into `1..=MAX_WAIT_MS`. The low end stops a
            // sub-millisecond remainder becoming a zero timeout, which
            // `SubmitIoRing` reads as "poll and return" and would turn the
            // tail of every wait into a spin. The high end keeps the waiter
            // from ever being handed `u32::MAX`, which is Win32's `INFINITE`
            // -- a `WaitForMultipleObjects`-based waiter would block forever
            // on a bound its caller believed was finite.
            let ms = u32::try_from(remaining.as_millis())
                .unwrap_or(MAX_WAIT_MS)
                .clamp(1, MAX_WAIT_MS);
            wait.wait(
                &mut RingWait {
                    handle: self.handle,
                    accounting: &self.accounting,
                },
                ms,
            )?;
        }
    }
}

/// How [`IoRing::pop_within_with`] blocks between checks of the completion
/// queue (M21.2).
///
/// Implement this to drive a bounded pop from a wait this crate does not own
/// -- a completion event the caller already holds, a multiplexed
/// `WaitForMultipleObjects`, or a pure spin on a thread that must not block
/// in the kernel.
pub trait CompletionWait {
    /// Block until a completion *may* be available, or `timeout_ms` elapses.
    ///
    /// **Returning early or spuriously is always permitted**, and requires no
    /// apology: [D-19](../DESIGN-NOTES.md#d-19) makes a wake with nothing to
    /// pop a normal event, so the caller re-checks the queue either way. An
    /// implementation therefore cannot be subtly wrong about *when* to
    /// return; it can only waste time or burn CPU.
    ///
    /// What it must not do is block past `timeout_ms`, because that is the
    /// only thing standing between a stuck ring and a hung process.
    ///
    /// # An expired wait is `Ok(())`, never an error
    ///
    /// This is the one part of the contract an implementation can get wrong
    /// silently, and the crate's own [`SubmitWait`] got it wrong first: Win32
    /// reports an expired wait as a *failure* code (`ERROR_TIMEOUT` from
    /// `SubmitIoRing`, `WAIT_TIMEOUT` from the `WaitFor*` family), so
    /// forwarding the underlying result verbatim turns every ordinary timeout
    /// into an `Err`. [`IoRing::pop_within`] then reports a timeout as a
    /// failure rather than as the `Ok(None)` it promises, and every caller
    /// that matches on `Ok(None)` to detect a timeout stops working.
    ///
    /// So: the bound running out is a **successful** wait that happened to
    /// observe nothing. Report the error only when the wait itself failed.
    ///
    /// `timeout_ms` is never zero and never `u32::MAX`, so it can be passed
    /// straight to a Win32 wait without being mistaken for "poll and return"
    /// or for `INFINITE`.
    ///
    /// # Errors
    ///
    /// Whatever the underlying wait reports as a genuine failure. An error
    /// ends the pop.
    fn wait(&mut self, ring: &mut RingWait<'_>, timeout_ms: u32) -> io::Result<()>;
}

/// The ring, narrowed to what a [`CompletionWait`] needs (M21.2).
///
/// Deliberately exposes no way to pop and no way to submit work. A waiter that
/// could pop would consume the completion its own caller is waiting for, and
/// one that could submit would queue entries the caller never asked for --
/// both of which a bare `&mut IoRing` would permit. This is the same
/// narrowing, for the same reason, as
#[cfg_attr(
    feature = "threadpool",
    doc = "[`RingScope`](crate::RingScope) under [D-43](../DESIGN-NOTES.md#d-43)."
)]
#[cfg_attr(
    not(feature = "threadpool"),
    doc = "`RingScope` (the default `threadpool` feature) under [D-43](../DESIGN-NOTES.md#d-43)."
)]
pub struct RingWait<'ring> {
    /// Narrowed to what a wait actually uses -- the handle to submit on, and
    /// the ledger to ask how much is outstanding -- rather than the whole
    /// ring. That is the same narrowing `M24.7` made for `Token::new`, and
    /// here it also keeps [`CompletionWait`] free of `IoRing`'s payload
    /// parameter: a waiter blocks on a ring, and what the ring is holding for
    /// its caller is none of its business.
    handle: *mut c_void,
    accounting: &'ring Accounting,
}

impl RingWait<'_> {
    /// Block in the ring's own wait for up to `timeout_ms`, queueing nothing.
    ///
    /// With no new entries of its own, `SubmitIoRing`'s effect here is to
    /// submit whatever is already queued and wait for an outstanding
    /// operation to complete -- the same call [`IoRing::run_down`] uses to
    /// quiesce. Note *submit*: a `Build*` that has queued an SQE but not yet
    /// submitted it will be submitted by this call, which is why the
    /// narrowing below is about not letting a waiter **build** work, not
    /// about suppressing submission.
    ///
    /// **An expired wait is `Ok`, not an error.** Returning therefore does
    /// not mean a completion is poppable -- the bound may simply have run out
    /// -- which is why the loop that calls this re-checks either way.
    ///
    /// # At least one operation must really be outstanding
    ///
    /// Measured while building this: `SubmitIoRing` answers
    /// `E_INVALIDARG` (`0x80070057`) -- not a timeout -- when asked to wait
    /// for a completion the kernel has no pending operation for. A `RingWait`
    /// is only ever constructed by [`IoRing::pop_within_with`], which checks
    /// [`IoRing::outstanding`] before consulting the wait, so that
    /// precondition holds structurally rather than by the caller remembering
    /// it. A waiter that wants to block some other way is free to ignore this
    /// method entirely.
    ///
    /// # Errors
    ///
    /// Any error from `SubmitIoRing` other than an expired wait.
    pub fn block(&mut self, timeout_ms: u32) -> io::Result<()> {
        let mut submitted = 0_u32;
        // SAFETY: the ring handle is live for the borrow, and the out-pointer
        // is valid. Zero new SQEs are queued, so this call's only effect is
        // to wait for and reap what is already outstanding.
        let hr = unsafe { crate::sys::submit(self.handle, 1, timeout_ms, &raw mut submitted) };
        wait_outcome(hr)
    }

    /// Operations submitted but not yet observed complete, as
    /// [`IoRing::outstanding`].
    ///
    /// A waiter that multiplexes several sources can use this to decide
    /// whether blocking on this ring is worth a slot at all.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.accounting.outstanding()
    }
}

/// The default [`CompletionWait`]: block inside the ring's own
/// `SubmitIoRing` wait (M21.2).
///
/// Costs no kernel object and touches no event, so it composes with a caller
/// who owns the ring's completion event under
/// [D-21](../DESIGN-NOTES.md#d-21).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubmitWait;

impl CompletionWait for SubmitWait {
    fn wait(&mut self, ring: &mut RingWait<'_>, timeout_ms: u32) -> io::Result<()> {
        ring.block(timeout_ms)
    }
}

#[cfg(test)]
impl IoRing {
    /// An `IoRing` that owns no kernel ring, for driving `Drop`'s two error
    /// paths (M23.5).
    ///
    /// The handle is null **deliberately and specifically**. Measured:
    /// `CloseIoRing(null)` and `crate::sys::submit(null, ..)` both return
    /// `0x80070006` -- `HRESULT_FROM_WIN32(ERROR_INVALID_HANDLE)` -- so both
    /// of `Drop`'s failure branches can be reached without a fault-injection
    /// seam over the raw HRESULTs, which is what M23.5 was opened to price.
    ///
    /// A *non-null* fabricated handle is not a substitute and must never be
    /// swapped in: `CloseIoRing` on a plausible-looking `0xDEAD_0000` raises
    /// `STATUS_ACCESS_VIOLATION`, because a ring handle is a pointer the
    /// kernel dereferences rather than an index into a handle table.
    ///
    /// Nothing here opens a ring, so the tests built on it are not part of the
    /// ring-opening population `tools/check-ring-tests.ps1` tracks (D-49) --
    /// they need the kernel only to refuse them, which costs no ring.
    fn refused_by_the_kernel() -> Self {
        Self {
            handle: std::ptr::null_mut(),
            version: RingVersion::V1,
            supported_ops: OpSupport::default(),
            accounting: Accounting::new(),
            registered_buffer_infos: Vec::new(),
            completion_event: None,
            inventory: ManuallyDrop::new(HashMap::new()),
        }
    }
}

impl<T, X> Drop for IoRing<T, X> {
    fn drop(&mut self) {
        // A count of how many times this body has run, so a test can confirm
        // the rundown-and-close actually executes rather than trusting the
        // impl exists. The counter is thread-local: a process-wide one is
        // incremented by every other test's rings as they drop, which would
        // let an `after > before` assertion be satisfied by somebody else's
        // drop and mask the very mutation it exists to catch.
        #[cfg(test)]
        DROP_RUNS.with(|runs| runs.set(runs.get() + 1));

        // Best-effort rundown: a ring with an operation still outstanding at
        // drop time is a use bug (M3's Batch/Token are the sanctioned way to
        // avoid it), but Drop cannot propagate the error, so this asserts in
        // debug builds rather than silently closing a ring the kernel may
        // still be writing through.
        //
        // Both asserts here are silent while already panicking (M23.4): a
        // second panic during unwind aborts, replacing whatever failure
        // started the unwind with `STATUS_STACK_BUFFER_OVERRUN`. A ring is
        // dropped on the way out of almost every failing test in this crate,
        // so an unguarded assert here would convert a readable assertion
        // message into a crash in the common case rather than a rare one.
        //
        // Both are covered, and neither needed a fault-injection seam to get
        // there (M23.5). `a_ring_whose_rundown_the_kernel_refuses_..` and
        // `a_ring_whose_close_the_kernel_refuses_..` put a null handle in the
        // field and let this body run for real: measured, `CloseIoRing(null)`
        // and `crate::sys::submit(null, ..)` both return `0x80070006`
        // (`ERROR_INVALID_HANDLE`). Whether `run_down` submits at all is what
        // selects between the two, since it loops only while something is
        // outstanding.
        //
        // A *non-null* bad handle is not equivalent and must never be
        // substituted: `CloseIoRing(0xDEAD_0000)` raises
        // `STATUS_ACCESS_VIOLATION`, because a ring handle is a pointer the
        // kernel dereferences rather than an index into a handle table.
        let quiesced = match self.run_down() {
            Ok(()) => true,
            Err(error) => {
                debug_assert!(
                    std::thread::panicking(),
                    "IoRing rundown failed before close: {error}"
                );
                false
            }
        };

        // The inventory is dropped only when rundown actually quiesced the
        // ring, and **forgotten otherwise** (D-73). This is `Token`'s
        // leak-on-unclaimed-drop, relocated: it used to be the caller's,
        // because the caller held the buffers and could not prove the kernel
        // was finished with them. The ring can prove it -- rundown returning
        // `Ok` is that proof -- but only on the path where rundown succeeds,
        // and the rundown above is deliberately best-effort. On the other path
        // the close below runs with operations possibly still outstanding, so
        // freeing what they point at would hand the kernel a dangling write.
        //
        // Leaking is the correct answer there, exactly as it is for a `Token`
        // dropped unclaimed: memory is lost, which is finite and visible,
        // rather than reused, which is neither.
        //
        // `is_quiescent` is rechecked rather than trusting `quiesced` alone so
        // that the free below and rundown's answer rest on the one predicate.
        // It is keyed by identity, so a CQE the ring never minted -- a
        // duplicate, or one carrying foreign user data -- cannot make it read
        // true while any operation, owned or raw, is still in flight.
        if quiesced && self.is_quiescent() {
            // SAFETY: nothing is outstanding, so no kernel write can still be
            // aimed at anything this holds, and `self.inventory` is not used
            // again -- this is `Drop`, and the field is `ManuallyDrop` so
            // nothing drops it a second time.
            unsafe { ManuallyDrop::drop(&mut self.inventory) };
        }
        // SAFETY: `self.handle` is a live ring this `IoRing` exclusively
        // owns, and `run_down` just established that nothing is outstanding
        // (or made a best-effort attempt to, above).
        let hr = unsafe { CloseIoRing(self.handle) };
        debug_assert!(
            hr >= 0 || std::thread::panicking(),
            "CloseIoRing failed: 0x{:08X}",
            hr as u32
        );
    }
}

// How many times `IoRing`'s `Drop` impl has run **on the calling thread**; see
// its use there.
//
// A plain comment rather than a doc comment: rustdoc does not document items
// produced by a macro invocation, so a doc comment here is an
// `unused_doc_comments` warning, which CI escalates with `-D warnings`.
//
// Thread-local rather than a shared `static`, and that is load-bearing rather
// than tidiness. `cargo test` runs tests as threads in one process, so a
// process-wide counter is incremented by every other test's rings as they
// drop -- and an assertion of the form `after > before` is then satisfied by
// *somebody else's* drop, which is exactly the mutant it was written to catch.
// A thread-local is only touched by rings dropped on this test's own thread.
#[cfg(test)]
thread_local! {
    pub(crate) static DROP_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Pop one completion, bounded, for tests that need a real one.
///
/// `Batch::submit_and_wait` returning does **not** mean a completion is
/// poppable -- its own documentation says so, because the timeout can expire
/// first. That leaves two ways for a test to be wrong, and this exists so
/// neither is spelled out at each call site.
///
/// A single `try_pop` flakes: under load the completion arrives just after the
/// check. A bare `loop` around `try_pop` is worse, because it converts that
/// flake into a hang -- and `cargo test` runs tests as threads in one process,
/// so a hung test stops the *whole harness* reporting and the failure arrives
/// with no test name attached. A bounded wait fails loudly instead, naming what
/// it waited for.
///
/// Thirty seconds matches the deadline the crate's own `failure_paths`
/// integration test already uses; it is a hang bound, not a latency
/// expectation, so it is far above any real completion time.
///
/// Since M21.2 this is a thin panicking wrapper over the public
/// [`IoRing::pop_within`] rather than its own loop. The panic is the only
/// thing left that is specific to tests: a test wants the name of what it
/// waited for in the failure message, where a consumer wants an `Option` it
/// can act on.
#[cfg(test)]
pub(crate) fn pop_within<T, X>(ring: &mut IoRing<T, X>, what: &str) -> Completion {
    // Named once so the bound and the message it reports cannot drift apart.
    const BOUND: std::time::Duration = std::time::Duration::from_secs(30);

    ring.pop_within(BOUND)
        .expect("pop")
        .unwrap_or_else(|| panic!("timed out after {BOUND:?} waiting for {what}"))
        .0
}

// Both are children rather than siblings so they can reach this module's
// private items -- `pop_raw`, `is_quiescent`, `drain_for_rundown` -- which stay
// here because `Drop` needs them too and a parent cannot see into its children.
// No public path changes: an inherent `impl` is found by its type.
mod bookkeeping;
mod inventory;

#[cfg(test)]
mod tests;
