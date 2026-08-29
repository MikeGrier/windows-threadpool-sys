// Copyright (c) 2026 Mike Grier
//! The owned `IoRing` handle (M1.2), and the op capability set (M1.4).

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Storage::FileSystem::{
    CloseIoRing, CreateIoRing, GetIoRingInfo, IORING_BUFFER_INFO, IORING_CQE,
    IORING_CREATE_ADVISORY_FLAGS_NONE, IORING_CREATE_FLAGS, IORING_CREATE_REQUIRED_FLAGS_NONE,
    IORING_INFO, IORING_OP_CANCEL, IORING_OP_CODE, IORING_OP_FLUSH, IORING_OP_NOP, IORING_OP_READ,
    IORING_OP_REGISTER_BUFFERS, IORING_OP_REGISTER_FILES, IORING_OP_WRITE, IsIoRingOpSupported,
    PopIoRingCompletion, SetIoRingCompletionEvent, SubmitIoRing,
};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

use crate::capability::{RingVersion, capabilities};
use crate::error::check;

/// A ring's identity, unique for the process's lifetime (PR #20 review
/// response): every value a ring hands out that later gets checked back
/// against it -- a [`crate::Token`], a [`crate::RegisteredFile`], a
/// [`crate::RegisteredBuffers`] -- carries the id of the ring that minted
/// it, and every [`Completion`] carries the id of the ring that popped it.
///
/// A monotonic counter rather than the ring's own `HANDLE`: a `HANDLE` is
/// only unique while the object it names is still open, and Windows is free
/// to hand a closed ring's numeric value to the *next* object created --
/// which would let a stale identity from a closed ring collide with a
/// brand-new one. This counter never repeats within one process run
/// (`u64` overflow is not a practical concern), so a mismatch always means
/// a genuine cross-ring mixup, never a false negative from handle reuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RingId(u64);

impl RingId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

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
    /// [`Completion::synthetic`] is the opposite, and is why it is not
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
    /// This ring's own identity (PR #20 review response); see [`RingId`].
    ring_id: RingId,
    /// The next `UserData` value [`IoRing::reserve_user_data`] will hand out.
    next_user_data: usize,
    /// Operations minted but not yet observed to have completed (M2.4).
    outstanding: usize,
    /// How many file handles are registered so far, across every confirmed
    /// `BuildIoRingRegisterFileHandles` (M5.1). The base index of the next
    /// registration.
    registered_files: u32,
    /// As `registered_files`, for `BuildIoRingRegisterBuffers` (M5.2).
    registered_buffers: u32,
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
            .field("ring_id", &self.ring_id)
            .field("next_user_data", &self.next_user_data)
            .field("outstanding", &self.outstanding)
            .field("registered_files", &self.registered_files)
            .field("registered_buffers", &self.registered_buffers)
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
            ring_id: RingId::next(),
            next_user_data: 0,
            outstanding: 0,
            registered_files: 0,
            registered_buffers: 0,
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
        // Already attached: hand back another duplicate rather than
        // attaching a second event, which would silently detach the first
        // (`SetIoRingCompletionEvent` replaces rather than adds). The
        // capability was necessarily verified on the call that attached it.
        if let Some(event) = &self.completion_event {
            return event.try_clone();
        }

        if !capabilities()?.supports_completion_event {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this system's IoRing does not report IORING_FEATURE_SET_COMPLETION_EVENT",
            ));
        }

        // Auto-reset (manual_reset = FALSE) per D-21, initially unsignalled
        // -- the deliberate setup signal is raised below, after the event is
        // attached and owned, so it cannot be missed or lost.
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
        let hr = unsafe { SetIoRingCompletionEvent(self.handle, event.as_raw_handle()) };
        // On failure `event` drops here, closing a handle the ring never
        // successfully referenced.
        check(hr)?;

        // Stored *before* signalling: from this point the ring owns the
        // event, so no later failure can drop it and leave the ring
        // signalling a closed (possibly recycled) handle.
        let event = self.completion_event.insert(event);

        // The one deliberate spurious wakeup (rule 2 above): a caller who
        // submitted before attaching would otherwise never be woken for that
        // backlog, since the queue never returns to empty to re-arm the edge.
        // SAFETY: `event` is a live event handle this ring owns.
        if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }

        event.try_clone()
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
        self.registered_files
    }

    /// As [`IoRing::registered_file_count`], for registered buffers (M5.2) --
    /// a **reserved** count, not a confirmed one, with the same two
    /// consequences (M10.3, D-31).
    #[must_use]
    pub fn registered_buffer_count(&self) -> u32 {
        self.registered_buffers
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
        self.registered_files = self.registered_files.saturating_add(count);
    }

    /// As [`IoRing::reserve_registered_files`], for registered buffers.
    pub(crate) fn reserve_registered_buffers(&mut self, count: u32) {
        self.registered_buffers = self.registered_buffers.saturating_add(count);
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
        self.outstanding
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
        let id = self.next_user_data;
        self.next_user_data = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("IoRing operation identity space exhausted"))?;
        self.outstanding += 1;
        Ok(id)
    }

    /// Record that one outstanding operation's completion has been observed
    /// (a real `IORING_CQE` was popped for it), whether or not a live
    /// [`crate::Token`] was still around to claim it.
    pub(crate) fn record_completion(&mut self) {
        self.outstanding = self.outstanding.saturating_sub(1);
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
        self.outstanding = self.outstanding.saturating_sub(1);
    }

    /// This ring's native handle, for `batch.rs`'s `Build*`/`Submit` calls.
    pub(crate) fn raw_handle(&self) -> *mut c_void {
        self.handle
    }

    /// This ring's own identity, for stamping onto every [`crate::Token`]/
    /// registration it mints and checking against on use (PR #20 review
    /// response); see [`RingId`].
    pub(crate) fn ring_id(&self) -> RingId {
        self.ring_id
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
    /// # Errors
    ///
    /// Returns any error from `SubmitIoRing` or `PopIoRingCompletion`.
    pub fn run_down(&mut self) -> io::Result<()> {
        while self.outstanding > 0 {
            let mut submitted = 0_u32;
            // SAFETY: `self.handle` is a live ring; valid out-pointer. Zero
            // new SQEs are queued -- this call's only purpose is to wait for
            // and reap already-outstanding completions.
            let hr = unsafe { SubmitIoRing(self.handle, 1, RUN_DOWN_POLL_MS, &raw mut submitted) };
            check(hr)?;
            self.drain_for_rundown()?;
        }
        Ok(())
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
        let hr = unsafe { PopIoRingCompletion(self.handle, &raw mut cqe) };
        if hr == S_FALSE {
            return Ok(None);
        }
        check(hr)?;
        self.record_completion();
        Ok(Some(Completion {
            user_data: cqe.UserData,
            result_code: cqe.ResultCode,
            information: cqe.Information,
            ring_id: self.ring_id,
        }))
    }
}

impl Drop for IoRing {
    fn drop(&mut self) {
        // Best-effort rundown: a ring with an operation still outstanding at
        // drop time is a use bug (M3's Batch/Token are the sanctioned way to
        // avoid it), but Drop cannot propagate the error, so this asserts in
        // debug builds rather than silently closing a ring the kernel may
        // still be writing through.
        if let Err(error) = self.run_down() {
            debug_assert!(false, "IoRing rundown failed before close: {error}");
        }
        // SAFETY: `self.handle` is a live ring this `IoRing` exclusively
        // owns, and `run_down` just established that nothing is outstanding
        // (or made a best-effort attempt to, above).
        let hr = unsafe { CloseIoRing(self.handle) };
        debug_assert!(hr >= 0, "CloseIoRing failed: 0x{:08X}", hr as u32);
    }
}

#[cfg(test)]
mod tests;
