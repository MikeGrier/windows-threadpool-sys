// Copyright (c) 2026 Mike Grier
//! The owned `IoRing` handle (M1.2), and the op capability set (M1.4).

use std::ffi::c_void;
use std::io;

use windows_sys::Win32::Storage::FileSystem::{
    CloseIoRing, CreateIoRing, GetIoRingInfo, IORING_CQE, IORING_CREATE_ADVISORY_FLAGS_NONE,
    IORING_CREATE_FLAGS, IORING_CREATE_REQUIRED_FLAGS_NONE, IORING_INFO, IORING_OP_CANCEL,
    IORING_OP_CODE, IORING_OP_FLUSH, IORING_OP_NOP, IORING_OP_READ, IORING_OP_REGISTER_BUFFERS,
    IORING_OP_REGISTER_FILES, IORING_OP_WRITE, IsIoRingOpSupported, PopIoRingCompletion,
    SubmitIoRing,
};

use crate::capability::{RingVersion, capabilities};
use crate::error::check;

/// One `IoRing` operation.
///
/// `#[non_exhaustive]`: the kernel's op table has grown before (M1.4, D-7)
/// and will again. A consumer must not be able to write an exhaustive
/// `match` that a new variant would break. [`IoRing::supports_raw`] reaches
/// an op this enum does not yet name.
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

/// One popped completion (M3.7): the operation's identity, from
/// `IORING_CQE::UserData`, and its result.
#[derive(Clone, Copy, Debug)]
pub struct Completion {
    user_data: usize,
    result_code: windows_sys::core::HRESULT,
    information: usize,
}

impl Completion {
    /// The `UserData` identity this completion reports -- match it against
    /// a held [`crate::Token`] via [`crate::Token::claim_if`].
    #[must_use]
    pub fn user_data(&self) -> usize {
        self.user_data
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
#[derive(Debug)]
pub struct IoRing {
    handle: *mut c_void,
    version: RingVersion,
    supported_ops: OpSupport,
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
            next_user_data: 0,
            outstanding: 0,
            registered_files: 0,
            registered_buffers: 0,
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
    #[must_use]
    pub fn supports(&self, op: Op) -> bool {
        self.supported_ops.contains(op)
    }

    /// Whether this ring supports a raw op code this crate does not yet name
    /// (D-7).
    ///
    /// Unlike [`IoRing::supports`], this is not cached -- it exists
    /// specifically for an op outside [`Op`], which by definition this
    /// ring's cached capability set was never probed for.
    #[must_use]
    pub fn supports_raw(&self, op_code: IORING_OP_CODE) -> bool {
        // SAFETY: `self.handle` is a live ring.
        unsafe { IsIoRingOpSupported(self.handle, op_code) != 0 }
    }

    /// How many file handles are registered on this ring so far, across
    /// every `BuildIoRingRegisterFileHandles` this crate has successfully
    /// queued (M5.1). This is the base index the next registration will
    /// start from -- see `reserve_registered_files` for why it advances
    /// eagerly rather than waiting for a completion (D-14).
    #[must_use]
    pub fn registered_file_count(&self) -> u32 {
        self.registered_files
    }

    /// As [`IoRing::registered_file_count`], for registered buffers (M5.2).
    #[must_use]
    pub fn registered_buffer_count(&self) -> u32 {
        self.registered_buffers
    }

    /// Advance the registered-file base index by `count`, the instant a
    /// `BuildIoRingRegisterFileHandles` call successfully queues (not once
    /// its completion is observed).
    ///
    /// Recorded as an explicitly unverified assumption (D-14, mirroring
    /// D-10 above): this crate does not know whether the kernel claims
    /// these `count` indices synchronously at build time or only once the
    /// registration op actually runs. Advancing eagerly is the safe
    /// direction either way -- it can only ever waste indices by advancing
    /// too early, never collide two registrations on the same index by
    /// advancing too late, which is the failure mode that would actually
    /// corrupt a later registration's base index.
    pub(crate) fn reserve_registered_files(&mut self, count: u32) {
        self.registered_files = self.registered_files.saturating_add(count);
    }

    /// As [`IoRing::reserve_registered_files`], for registered buffers.
    pub(crate) fn reserve_registered_buffers(&mut self, count: u32) {
        self.registered_buffers = self.registered_buffers.saturating_add(count);
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
