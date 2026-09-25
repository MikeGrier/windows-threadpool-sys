// Copyright (c) 2026 Mike Grier
//! The owned `IoRing` handle (M1.2), and the op capability set (M1.4).

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
    /// The `UserData` identity this completion reports -- match it against
    /// a held [`crate::Token`] via [`crate::Token::claim_if`].
    #[must_use]
    pub fn user_data(&self) -> usize {
        self.user_data
    }

    /// The identity of the ring that popped this completion (PR #20 review
    /// response): a [`crate::Token`]/registration only ever matches a
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
    /// [`crate::Token::claim_if`]'s safety argument is that a `Completion` for
    /// some `UserData` **existing at all** proves the kernel has finished with
    /// that operation, and therefore that handing its buffer back is sound.
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
    /// The soundness argument above is about [`crate::Token::claim_if`], where
    /// a failed completion changes nothing the caller does with memory: the
    /// buffer comes back either way. **A registration claim is different.**
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
    /// let completion = completion.with_injected_failure(
    ///     InjectedFailure::Win32(ERROR_ACCESS_DENIED),
    /// );
    /// assert!(completion.result().is_err());
    /// // The token still claims it: the operation did complete.
    /// let buffer = token.claim_if(&completion).expect("claims its own");
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

    /// Build a `Completion` without popping a real one, for tests that
    /// exercise [`crate::Token::claim_if`] without real I/O.
    ///
    /// Not available outside `#[cfg(test)]`: production code has no
    /// legitimate reason to fabricate a completion, since `Token::claim_if`'s
    /// whole safety argument depends on every `Completion` in existence
    /// tracing back to a real `IORING_CQE` `IoRing::try_pop` observed.
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
pub struct IoRing {
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
}

impl std::fmt::Debug for IoRing {
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
unsafe impl Send for IoRing {}

impl IoRing {
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
        })
    }

    /// The version this ring was created at.
    #[must_use]
    pub fn version(&self) -> RingVersion {
        self.version
    }

    /// Query this ring's current info via `GetIoRingInfo`.
    ///
    /// # Errors
    ///
    /// Returns any error from `GetIoRingInfo`.
    pub fn info(&self) -> io::Result<RingInfo> {
        let mut raw = IORING_INFO::default();
        // SAFETY: `self.handle` is a live ring; `raw` is a valid out-pointer.
        let hr = unsafe { GetIoRingInfo(self.handle, &raw mut raw) };
        check(hr)?;
        Ok(RingInfo {
            version: RingVersion::from_raw(raw.IoRingVersion),
            submission_queue_size: raw.SubmissionQueueSize,
            completion_queue_size: raw.CompletionQueueSize,
        })
    }

    /// Whether this ring supports `op`, from the capability set cached at
    /// construction.
    ///
    /// Answers what the *kernel's* op table contains, not what this crate's
    /// safe push surface reaches (M10.1, [`Op`]'s own docs list the mapping).
    /// The two coincide for every op except [`Op::Nop`], which has no
    /// [`crate::Batch`] method at all: a nop owns no buffer, so there is
    /// nothing for a [`crate::Token`] to hand back, and it is reachable only
    /// through [`IoRing::push_raw`]. A `true` here therefore means "the
    /// kernel would accept this op", not "a `Batch` method exists to push
    /// it".
    #[must_use]
    pub fn supports(&self, op: Op) -> bool {
        self.supported_ops.contains(op)
    }

    /// Overrides the cached capability set to exactly `ops`, for tests that
    /// need a ring known to lack support for something.
    ///
    /// Every real host this crate has been tested against supports all seven
    /// named ops, which is exactly why [`IoRing::supports`] and
    /// [`Batch::require`](crate::Batch)'s use of it could not be told apart
    /// from a constant `true` by any test that only ever asked a real ring:
    /// the honest answer and the constant agree on every host available to
    /// run the test. This seam constructs the disagreement instead of hoping
    /// to find a host that has it.
    ///
    /// Not available outside `#[cfg(test)]`, for the same reason
    /// [`Completion::synthetic`] is not: production code has no legitimate
    /// reason to claim a capability the kernel did not actually report.
    #[cfg(test)]
    pub(crate) fn set_supported_ops_for_test(&mut self, ops: &[Op]) {
        self.supported_ops = OpSupport(ops.iter().fold(0_u8, |mask, &op| {
            let index = Op::ALL
                .iter()
                .position(|&candidate| candidate == op)
                .expect("Op::ALL is exhaustive");
            mask | (1 << index)
        }));
    }

    /// An owned duplicate of this ring's completion event, so a caller can
    /// wait on the ring alongside other handles without surrendering it
    /// (M11.1, D-20).
    ///
    /// The ring creates the event, attaches it with
    /// `SetIoRingCompletionEvent`, keeps ownership, and hands back a
    /// duplicate. Closing the returned handle is therefore always safe: the
    /// ring keeps signalling its own copy, and a `DuplicateHandle`'d event is
    /// still signalled after the original is closed.
    ///
    /// **Idempotent.** Repeat calls return another duplicate of the *same*
    /// event rather than attaching a new one, so two subsystems can each ask
    /// without silently detaching the other's.
    ///
    /// This is not a third delivery architecture -- it is Model B with a
    /// multiplexed wakeup source, changing only what the domain thread blocks
    /// on, never who owns, submits, or drains ([`Batch::submit_and_wait`] is
    /// still the single-source shape). Use it when ring I/O has to be waited
    /// on alongside non-ring I/O, which
    /// `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` cannot order across.
    ///
    /// [`Batch::submit_and_wait`]: crate::Batch::submit_and_wait
    ///
    /// # The event is an edge, not a level
    ///
    /// **Measured, not inferred, and getting it wrong hangs rather than just
    /// slowing down** (D-19). The event is signalled when the completion
    /// queue transitions from **empty to non-empty**. It is *not* signalled
    /// once per completion, and it is *not* level-triggered: eight
    /// completions arriving at once into an empty queue produce exactly one
    /// wakeup, and a completion arriving into an already-non-empty queue
    /// produces none.
    ///
    /// Two rules follow, and they are contract rather than advice:
    ///
    /// 1. **Drain to empty before waiting again** -- [`IoRing::try_pop`]
    ///    until it yields `None`, on *every* pass through a multiplexed wait
    ///    loop, not only the pass where this handle signalled. A wait entered
    ///    with entries still in the queue blocks until some later completion
    ///    arrives *after* the queue has been emptied, which may be never.
    ///    That is a lost-wakeup deadlock, not a latency wobble.
    /// 2. **A wake with nothing to pop is normal.** It must not be treated as
    ///    an error or as a spurious wakeup. This method deliberately produces
    ///    one: the event is signalled once before it returns, so a caller who
    ///    had already submitted work never misses a backlog that arrived
    ///    before the event was attached.
    ///
    /// The event is auto-reset, and **exactly one waiter per ring** is
    /// supported (D-21): the drain that restores the empty state, and so
    /// re-arms the edge, must run to empty exactly once. Two threads waiting
    /// on one ring's event cannot be made correct.
    ///
    /// # If you are arming a thread-pool wait on this handle
    ///
    /// `SetThreadpoolWait` documents that "you must re-register the event
    /// with the wait object before signaling it each time to trigger the wait
    /// callback". This method signals the event *before* it returns (rule 2
    /// above), so by the time you have a handle to build a wait object
    /// around, that setup signal has already happened -- in the order the
    /// rule forbids. It is not guaranteed to run your callback, and because
    /// the event is auto-reset the signal is *consumed* rather than left
    /// pending, so a later arming has nothing to observe. Combined with the
    /// edge rule above, a ring whose queue never returns to empty has no
    /// second wakeup coming: the loss is permanent, not late.
    ///
    /// The remedy needs nothing this method does not already give you --
    /// after arming the wait, signal your own duplicate yourself:
    ///
    /// ```no_run
    /// # use windows_ioring_sys::IoRing;
    /// # use std::os::windows::io::AsRawHandle;
    /// # fn f(ring: &mut IoRing) -> std::io::Result<()> {
    /// let event = ring.completion_event()?;
    /// // ... build the wait object around `event`, then arm it ...
    /// // Only now raise the wakeup, in the documented order. A wake with
    /// // nothing to pop is normal (rule 2), so this is always safe.
    /// unsafe { windows_sys::Win32::System::Threading::SetEvent(event.as_raw_handle()) };
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// [`EventDelivery`](crate::EventDelivery) already does this for you and
    /// is the better answer if you do not need the handle itself.
    ///
    /// `examples/model_b_multiplexed.rs` is this whole shape worked end to
    /// end -- a caller-owned ring waited on alongside a shutdown latch, with
    /// the quiesce that shutdown-while-outstanding requires (M11.6).
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if the running system does not
    /// report `IORING_FEATURE_SET_COMPLETION_EVENT`; this crate refuses to
    /// silently substitute a polling thread, since a caller who asked for an
    /// event and got a spun-up thread has been told something false. Also
    /// returns any error from `CreateEventW`,
    /// `SetIoRingCompletionEvent`, `SetEvent`, or duplicating the handle.
    pub fn completion_event(&mut self) -> io::Result<OwnedHandle> {
        let (event, owes_setup_signal) = self.attach_completion_event_unsignalled()?;
        if owes_setup_signal {
            self.raise_setup_signal()?;
        }
        Ok(event)
    }

    /// Attach the ring's completion event *without* raising the setup signal,
    /// reporting whether that signal is still owed.
    ///
    /// `SetThreadpoolWait` documents that "you must re-register the event with
    /// the wait object before signaling it each time to trigger the wait
    /// callback". A caller that is about to arm a threadpool wait on this
    /// event therefore needs the attachment and the signal as two steps, so
    /// that arming can be sequenced between them; handing back an
    /// already-signalled event leaves that caller no way to obey the rule.
    /// [`IoRing::completion_event`] is these two composed, for a caller who
    /// does its own waiting and is not bound by that rule.
    ///
    /// The flag is false when the ring already had an event attached, which
    /// matches [`IoRing::completion_event`]: the setup signal belongs to the
    /// call that performs the attachment.
    pub(crate) fn attach_completion_event_unsignalled(
        &mut self,
    ) -> io::Result<(OwnedHandle, bool)> {
        // Already attached: hand back another duplicate rather than
        // attaching a second event, which would silently detach the first
        // (`SetIoRingCompletionEvent` replaces rather than adds). The
        // capability was necessarily verified on the call that attached it.
        if let Some(event) = &self.completion_event {
            return Ok((event.try_clone()?, false));
        }

        if !capabilities()?.supports_completion_event {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this system's IoRing does not report IORING_FEATURE_SET_COMPLETION_EVENT",
            ));
        }

        // Auto-reset (manual_reset = FALSE) per D-21, initially unsignalled
        // -- the deliberate setup signal is raised by `raise_setup_signal`,
        // after the event is attached and owned, so it cannot be lost, and
        // after any threadpool wait has been armed, so the arm-before-signal
        // rule `SetThreadpoolWait` documents is obeyed.
        // SAFETY: null attributes and name are documented defaults.
        let raw = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `CreateEventW` just returned this handle and it is not
        // owned anywhere else, so `OwnedHandle` is its sole owner from here.
        let event = unsafe { OwnedHandle::from_raw_handle(raw) };

        // SAFETY: `self.handle` is a live ring; `event` is a live event that
        // this ring will own for the rest of its life once stored below.
        let hr = unsafe { crate::sys::set_completion_event(self.handle, event.as_raw_handle()) };
        // On failure `event` drops here, closing a handle the ring never
        // successfully referenced.
        check(hr)?;

        // Stored *before* the setup signal can be raised: from this point the
        // ring owns the event, so no later failure can drop it and leave the
        // ring signalling a closed (possibly recycled) handle.
        self.completion_event
            .insert(event)
            .try_clone()
            .map(|dup| (dup, true))
    }

    /// Raise the one deliberate setup signal on the attached completion event.
    ///
    /// This is the single spurious wakeup the event's contract allows for: a
    /// caller who submitted before attaching would otherwise never be woken
    /// for that backlog, since the queue never returns to empty and so never
    /// re-arms the edge (D-19).
    ///
    /// Separated from the attachment so that a caller arming a threadpool wait
    /// can obey `SetThreadpoolWait`'s documented ordering -- register first,
    /// signal second. Does nothing if no event is attached, which cannot
    /// happen on the paths that call it.
    pub(crate) fn raise_setup_signal(&self) -> io::Result<()> {
        let Some(event) = &self.completion_event else {
            return Ok(());
        };
        // SAFETY: `event` is a live event handle this ring owns.
        if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // The setup signal is what a waiter attaching to a backlog depends on
        // entirely, so M26.9's investigation needs to know it happened -- and
        // that it happened on the ring's own handle rather than a duplicate
        // handed to a caller, since only the former is what the kernel will go
        // on signalling.
        //
        // Gated because `windows-threadpool-sys` is optional (D-22): this
        // method is on the always-present path, unlike the delivery module,
        // so an ungated reference breaks `--no-default-features`.
        #[cfg(feature = "threadpool")]
        windows_threadpool_sys::trace_record!(
            "delivery",
            "setup-signalled",
            event.as_raw_handle() as usize,
            self.accounting.outstanding()
        );
        Ok(())
    }

    /// Whether this ring supports a raw op code, including one this crate
    /// does not yet name (D-7).
    ///
    /// Its reason to exist is an op outside [`Op`], which by definition this
    /// ring's cached capability set was never probed for -- but passing a
    /// named op's [`Op::code`] is equally in contract, and answers
    /// identically to [`IoRing::supports`] (M10.1). The difference between
    /// them is cost, not truth: `supports` is a bit test against the set
    /// probed once at construction, this is an `IsIoRingOpSupported` call
    /// every time.
    ///
    /// What a `true` here does *not* mean is that the op became pushable: an
    /// op outside [`Op`] has no builder method whatever this answers, so
    /// [`IoRing::push_raw`] remains the only route to one.
    #[must_use]
    pub fn supports_raw(&self, op_code: IORING_OP_CODE) -> bool {
        // SAFETY: `self.handle` is a live ring.
        unsafe { IsIoRingOpSupported(self.handle, op_code) != 0 }
    }

    /// How many file handles this ring has **reserved** for registration --
    /// not how many are confirmed registered (M5.1, M10.3, D-31).
    ///
    /// The count advances the instant a `BuildIoRingRegisterFileHandles`
    /// call queues, never when its completion is observed. Two consequences
    /// a caller must not be surprised by:
    ///
    /// - it is already advanced before any completion has been popped, so it
    ///   cannot be used to decide whether a registration has taken effect --
    ///   claim the completion with
    ///   [`crate::PendingFileRegistration::claim_if`] for that;
    /// - it stays advanced after a registration whose completion reported
    ///   *failure*, which is why such a registration cannot be retried on
    ///   this ring ([`crate::Batch::register_files`]).
    ///
    /// Because a ring accepts at most one registration that assigns an
    /// index, this is `0` until that registration is queued and its count
    /// thereafter; there is no second registration for it to serve as a base
    /// index for.
    #[must_use]
    pub fn registered_file_count(&self) -> u32 {
        self.accounting.registered_file_count()
    }

    /// As [`IoRing::registered_file_count`], for registered buffers (M5.2) --
    /// a **reserved** count, not a confirmed one, with the same two
    /// consequences (M10.3, D-31).
    #[must_use]
    pub fn registered_buffer_count(&self) -> u32 {
        self.accounting.registered_buffer_count()
    }

    /// Advance the registered-file base index by `count`, the instant a
    /// `BuildIoRingRegisterFileHandles` call successfully queues (not once
    /// its completion is observed).
    ///
    /// D-14 recorded this as an explicitly unverified assumption, since this
    /// crate cannot know whether the kernel claims these `count` indices
    /// synchronously at build time or only once the registration op runs.
    /// D-31 (M10.3) dissolved that: the collision it guarded against needs a
    /// *second* registration, and `Batch::register_files`/`register_buffers`
    /// forbid one, so no later base index is ever derived from this count and
    /// the kernel's actual timing has no observable consequence. What the
    /// eager advance does still determine is the *meaning* of the public
    /// accessors, which is why they document a reserved rather than a
    /// confirmed count.
    ///
    /// D-32 did not answer this question, despite being adjacent to it: it
    /// established when the kernel reads the `IORING_BUFFER_INFO` *array*,
    /// which is a different thing from when it claims the *indices*. The
    /// latter remains unmeasured, and dissolved rather than resolved.
    pub(crate) fn reserve_registered_files(&mut self, count: u32) {
        self.accounting.reserve_registered_files(count);
    }

    /// As [`IoRing::reserve_registered_files`], for registered buffers.
    pub(crate) fn reserve_registered_buffers(&mut self, count: u32) {
        self.accounting.reserve_registered_buffers(count);
    }

    /// Take ownership of the `IORING_BUFFER_INFO` array a
    /// `BuildIoRingRegisterBuffers` call was handed, keeping it alive for
    /// this ring's remaining life, and hand back a stable pointer to it
    /// (D-32).
    ///
    /// The kernel reads this array when the registration op *runs*, not when
    /// the `Build*` call returns, so the caller must not build the SQE from a
    /// temporary: store the array here first and pass the returned pointer.
    /// Returns a null pointer for an empty array, which is what
    /// `BuildIoRingRegisterBuffers` should be handed for a zero-length
    /// registration anyway.
    pub(crate) fn hold_registered_buffer_infos(
        &mut self,
        infos: Vec<IORING_BUFFER_INFO>,
    ) -> *const IORING_BUFFER_INFO {
        debug_assert!(
            self.registered_buffer_infos.is_empty(),
            "a ring accepts at most one buffer registration, so this must only be set once"
        );
        self.registered_buffer_infos = infos;
        // `Vec::as_ptr` is stable for as long as the `Vec` is neither moved
        // out of nor reallocated; it lives in `self` and is never mutated
        // again, and moving the `IoRing` itself moves only the `Vec` header,
        // not its heap allocation.
        self.registered_buffer_infos.as_ptr()
    }

    /// How many operations this ring believes are still outstanding: minted
    /// (via `reserve_user_data`) but not yet observed to have completed (via
    /// `record_completion`).
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.accounting.outstanding()
    }

    /// Mint a fresh `UserData` identity for a new operation, and account for
    /// it as outstanding until `record_completion` is called for it.
    ///
    /// The identity is the whole of what a [`crate::Token`] needs to validate
    /// a completion (D-4): unlike `windows-overlapped-io-sys`'s
    /// `OperationId`, there is no separate storage address to pair it with,
    /// because `UserData` is a value this crate chooses rather than one Win32
    /// hands back.
    ///
    /// # Errors
    ///
    /// Returns an error rather than reusing an identity if the `usize` space
    /// is ever exhausted, mirroring `windows-threadpool-sys`'s own
    /// "exhausting the generation sequence fails rather than wraps."
    pub(crate) fn reserve_user_data(&mut self) -> io::Result<usize> {
        self.accounting.reserve_user_data()
    }

    /// Record that one outstanding operation's completion has been observed
    /// (a real `IORING_CQE` was popped for it), whether or not a live
    /// [`crate::Token`] was still around to claim it.
    pub(crate) fn record_completion(&mut self) {
        self.accounting.record_completion();
    }

    /// Release a reservation for an operation that was never actually
    /// queued -- a `Build*` call failed synchronously, after
    /// [`IoRing::reserve_user_data`] had already minted its identity.
    ///
    /// Distinct from [`IoRing::record_completion`]: that marks a real
    /// `IORING_CQE` observed; this marks one that will never arrive because
    /// the op never entered the queue, so it must not count against
    /// [`IoRing::run_down`] either.
    pub(crate) fn cancel_reservation(&mut self) {
        self.accounting.cancel_reservation();
    }

    /// This ring's ledger, for the crate's own minting paths (M24.2).
    ///
    /// Handed out rather than proxied so that a `Token` can be minted from
    /// the bookkeeping alone -- which is what lets `token.rs`'s tests run
    /// without a kernel ring, since minting is all they ever needed one for.
    pub(crate) fn accounting_mut(&mut self) -> &mut Accounting {
        &mut self.accounting
    }

    /// This ring's native handle, for `batch.rs`'s `Build*`/`Submit` calls.
    pub(crate) fn raw_handle(&self) -> *mut c_void {
        self.handle
    }

    /// This ring's own identity, for stamping onto every [`crate::Token`]/
    /// registration it mints and checking against on use (PR #20 review
    /// response); see [`RingId`].
    pub(crate) fn ring_id(&self) -> RingId {
        self.accounting.ring_id()
    }

    /// Queue a raw, not-yet-wrapped SQE via a caller-supplied `Build*` call
    /// (M3.5, D-7).
    ///
    /// `build` receives this ring's native handle and a freshly reserved
    /// `UserData` value, and must call exactly one `BuildIoRing*` function
    /// with them, returning its `HRESULT`. On success the `UserData` is
    /// returned so the caller can match it against a later [`Completion`]
    /// popped by [`IoRing::try_pop`]; on failure the reservation is
    /// released, since the op was never actually queued.
    ///
    /// # Errors
    ///
    /// Returns any error `build`'s `HRESULT` reports.
    ///
    /// # Safety
    ///
    /// `build` must queue an SQE for a *self-contained* op: everything the
    /// kernel reads or writes for it must stay valid until the
    /// corresponding completion is observed, and this crate cannot verify
    /// what `build` does with the handle it is given. This is the same
    /// framing as `windows-overlapped-io-sys`'s raw `ioctl` seam
    /// (`device.rs`): the mechanics of building an SQE need nothing unsafe,
    /// but this crate cannot audit an arbitrary `Build*` call, so the seam
    /// itself is unsafe.
    pub unsafe fn push_raw(
        &mut self,
        build: impl FnOnce(*mut c_void, usize) -> windows_sys::core::HRESULT,
    ) -> io::Result<usize> {
        let user_data = self.reserve_user_data()?;
        let hr = build(self.handle, user_data);
        if let Err(error) = check(hr) {
            self.cancel_reservation();
            return Err(error);
        }
        Ok(user_data)
    }

    /// Block until every outstanding operation has completed, so
    /// `CloseIoRing` never runs while the kernel might still be touching a
    /// token's buffer.
    ///
    /// Waits in short, rechecked steps via `SubmitIoRing`'s own wait -- with
    /// zero new entries queued, its only effect is to block for up to
    /// `RUN_DOWN_POLL_MS` and reap whatever is already outstanding -- rather
    /// than one unbounded call. This does not interpret what it pops; M3/M4
    /// add the typed completion path `Token` consumes. Idempotent: calling it
    /// again once `outstanding() == 0` is a no-op.
    ///
    /// # A poll that expires is not a failure
    ///
    /// Each poll blocks for `RUN_DOWN_POLL_MS` and then reports
    /// `ERROR_TIMEOUT` if nothing finished in that window, which is the
    /// ordinary outcome for any operation slower than 50ms. Treating that as
    /// an error -- which this did until M21.6 -- made `run_down` return `Err`
    /// with the operation still outstanding, so `Drop` asserted and then
    /// called `CloseIoRing` anyway: exactly the "the kernel may still be
    /// writing through a token's buffer" hazard this function exists to
    /// prevent.
    ///
    /// This loop therefore has no overall bound, and that is deliberate.
    /// **Every SQE that successfully queues produces exactly one completion**
    /// (M10.2), so it terminates. Blocking until that holds is the safe
    /// failure mode; closing the ring early is not.
    ///
    /// # Choosing an unbounded wait is the caller's to make
    ///
    /// This spelling waits until rundown finishes, however long that takes,
    /// and calling it is how a caller elects that. A caller who wants to
    /// decide for themselves -- a deadline, a backoff, a number of attempts
    /// before giving up -- calls [`IoRing::run_down_within`] instead and owns
    /// the policy entirely. **This crate does not implement retry policy**
    /// (M26.8): it supplies a bounded primitive and reports what happened.
    ///
    /// Note the difference from [`IoRing::pop_within`], which also waits in
    /// segments: there the segments sit *inside a period the caller supplied*,
    /// which is a bounded wait implemented properly rather than a policy. This
    /// method had segments and no such period, which is what made it the one
    /// waiting API in this crate shaped wrongly.
    ///
    /// # Errors
    ///
    /// Returns any error from `SubmitIoRing` other than an expired wait, or
    /// from `PopIoRingCompletion`. **An error leaves the ring resumable**: see
    /// [`IoRing::run_down_within`] for what is guaranteed about the operations
    /// still queued.
    pub fn run_down(&mut self) -> io::Result<()> {
        while !self.run_down_within(Duration::MAX)? {}
        Ok(())
    }

    /// Run down for at most `bound`, reporting whether it finished.
    ///
    /// `Ok(true)` means nothing is outstanding and the ring is safe to drop.
    /// `Ok(false)` means the bound elapsed with work still in flight -- call
    /// again when your own policy says to. [`IoRing::outstanding`] says how
    /// much is left.
    ///
    /// # Why this exists, and why it returns rather than retries
    ///
    /// Rundown can fail for a reason that a later attempt would survive, and
    /// **deciding whether to make that attempt is not this crate's business**.
    /// A caller running under a deadline, a supervisor with a backoff, and a
    /// test that wants to fail fast all want different answers, and a policy
    /// baked in here would be wrong for two of the three. So this waits for
    /// exactly as long as it is told and then reports.
    ///
    /// # What an error guarantees, which is what makes retrying safe
    ///
    /// `SubmitIoRing` documents that *"If this function returns an error other
    /// than IORING_E_WAIT_TIMEOUT, then all entries remain in the submission
    /// queue."* So a failure here has **not** lost the operations and has not
    /// rewound them ([D-5](../DESIGN-NOTES.md#d-5)); they are still ring state,
    /// a later submit is what runs them, and their buffers must stay alive
    /// until they complete.
    ///
    /// The consequence worth stating plainly: after an `Err`, **do not drop
    /// this ring**. Dropping it closes a ring the kernel may still write
    /// through, which is the hazard rundown exists to prevent. Call again.
    ///
    /// # Errors
    ///
    /// Returns any error from `SubmitIoRing` other than an expired wait, or
    /// from `PopIoRingCompletion`.
    pub fn run_down_within(&mut self, bound: Duration) -> io::Result<bool> {
        let deadline = Instant::now().checked_add(bound);
        loop {
            if self.accounting.outstanding() == 0 {
                return Ok(true);
            }
            // `checked_add` rather than `+`, for the reason `pop_within_with`
            // records: `Instant + Duration` panics on overflow, so
            // `Duration::MAX` -- the honest spelling of "no deadline" -- would
            // take down the process. A deadline the clock cannot represent is
            // one that never arrives, which is what was asked for.
            let remaining = match deadline {
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                None => Duration::MAX,
            };
            if remaining.is_zero() {
                return Ok(false);
            }
            // Segments sit inside the caller's period, never outside it: the
            // poll is the shorter of the rundown step and what is left.
            let poll_ms = u32::try_from(remaining.as_millis())
                .unwrap_or(u32::MAX)
                .clamp(1, RUN_DOWN_POLL_MS);
            let mut submitted = 0_u32;
            // SAFETY: `self.handle` is a live ring; valid out-pointer. Zero
            // new SQEs are queued -- this call's only purpose is to wait for
            // and reap already-outstanding completions.
            let hr = unsafe { crate::sys::submit(self.handle, 1, poll_ms, &raw mut submitted) };
            wait_outcome(hr)?;
            self.drain_for_rundown()?;
        }
    }

    /// Pop every currently available completion, recording each -- without
    /// interpreting it, since rundown only needs to know a completion
    /// happened, not what it was.
    fn drain_for_rundown(&mut self) -> io::Result<()> {
        while self.try_pop()?.is_some() {}
        Ok(())
    }

    /// Pop one completion if the queue has one ready, without blocking
    /// (M3.7).
    ///
    /// Every popped completion is recorded via `record_completion`
    /// regardless of whether the caller still holds a [`crate::Token`] for
    /// it (D-4): accounting is driven by observing a real `IORING_CQE`,
    /// never by a token being dropped.
    ///
    /// `None` says the completion queue is empty *at this instant*, never
    /// that an operation will not complete. **Every SQE that successfully
    /// queues produces exactly one completion** (M10.2) -- unconditionally,
    /// which is what lets [`IoRing::run_down`] terminate. The only push that
    /// yields no completion is one whose `Build*` call failed synchronously,
    /// and that push's reservation is released rather than left outstanding.
    ///
    /// A popped completion matching no live [`crate::Token`] is **normal**,
    /// not a bug, and a drain loop must not treat it as one. It happens for
    /// a registration (claimed by [`crate::PendingFileRegistration`] or
    /// [`crate::PendingBufferRegistration`] instead), for a
    /// [`crate::Batch::flush_raw`]/[`crate::Batch::cancel_raw`] push (which
    /// return a bare identity because they own no buffer), for a cancel's own
    /// completion as distinct from its target's, and for a token the caller
    /// dropped unclaimed.
    ///
    /// # Errors
    ///
    /// Returns any error from `PopIoRingCompletion` other than its
    /// documented empty-queue result.
    pub fn try_pop(&mut self) -> io::Result<Option<Completion>> {
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
        self.record_completion();
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
    pub fn pop_within(&mut self, timeout: Duration) -> io::Result<Option<Completion>> {
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
    pub fn pop_within_with<W: CompletionWait + ?Sized>(
        &mut self,
        wait: &mut W,
        timeout: Duration,
    ) -> io::Result<Option<Completion>> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if let Some(completion) = self.try_pop()? {
                return Ok(Some(completion));
            }
            // Checked *after* the pop, never before: `record_completion` runs
            // during `try_pop`, so reading it first would race the very
            // completion being drained.
            if self.outstanding() == 0 {
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
            wait.wait(&mut RingWait { ring: self }, ms)?;
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
/// [`RingScope`](crate::RingScope) under [D-43](../DESIGN-NOTES.md#d-43).
pub struct RingWait<'ring> {
    ring: &'ring mut IoRing,
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
        let hr = unsafe { crate::sys::submit(self.ring.handle, 1, timeout_ms, &raw mut submitted) };
        wait_outcome(hr)
    }

    /// Operations submitted but not yet observed complete, as
    /// [`IoRing::outstanding`].
    ///
    /// A waiter that multiplexes several sources can use this to decide
    /// whether blocking on this ring is worth a slot at all.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.ring.outstanding()
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
        }
    }
}

impl Drop for IoRing {
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
        if let Err(error) = self.run_down() {
            debug_assert!(
                std::thread::panicking(),
                "IoRing rundown failed before close: {error}"
            );
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
pub(crate) fn pop_within(ring: &mut IoRing, what: &str) -> Completion {
    // Named once so the bound and the message it reports cannot drift apart.
    const BOUND: std::time::Duration = std::time::Duration::from_secs(30);

    ring.pop_within(BOUND)
        .expect("pop")
        .unwrap_or_else(|| panic!("timed out after {BOUND:?} waiting for {what}"))
}

#[cfg(test)]
mod tests;
