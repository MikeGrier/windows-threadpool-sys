// Copyright (c) 2026 Mike Grier
//! The owned `IoRing` handle (M1.2), and the op capability set (M1.4).

use std::ffi::c_void;
use std::io;

use windows_sys::Win32::Storage::FileSystem::{
    CloseIoRing, CreateIoRing, GetIoRingInfo, IORING_CREATE_ADVISORY_FLAGS_NONE,
    IORING_CREATE_FLAGS, IORING_CREATE_REQUIRED_FLAGS_NONE, IORING_INFO, IORING_OP_CANCEL,
    IORING_OP_CODE, IORING_OP_FLUSH, IORING_OP_NOP, IORING_OP_READ, IORING_OP_REGISTER_BUFFERS,
    IORING_OP_REGISTER_FILES, IORING_OP_WRITE, IsIoRingOpSupported,
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
}

impl Drop for IoRing {
    fn drop(&mut self) {
        // SAFETY: `self.handle` is a live ring this `IoRing` exclusively
        // owns, with no operations outstanding (M2 will add rundown before
        // this point once operations exist).
        let hr = unsafe { CloseIoRing(self.handle) };
        debug_assert!(hr >= 0, "CloseIoRing failed: 0x{:08X}", hr as u32);
    }
}

#[cfg(test)]
mod tests;
