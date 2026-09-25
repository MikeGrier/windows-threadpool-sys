// Copyright (c) Mike Grier
//! The seam the kernel-response resolver sits under (M26.2).
//!
//! Every `IoRing` FFI call that carries an *operation* goes through a wrapper
//! here instead of calling `windows-sys` directly. With the `kernel-seam`
//! feature off -- which is the published configuration and the default -- each
//! wrapper is an `#[inline(always)]` forward to the same call the crate made
//! before, and nothing in this module exists at all. With the feature on, a
//! test may install a [`Responses`] implementation that answers instead.
//!
//! # Why a module of wrappers rather than a generic `IoRing<K>`
//!
//! A kernel trait with `IoRing` generic over it is the shape this would
//! normally take, and it is ruled out by a collision rather than by taste:
//! [D-55](../DESIGN-NOTES.md#d-55) already spends `IoRing`'s type parameter on
//! the pending-token inventory (`M28.3`). Taking a second one would publish
//! `IoRing<T, K>` -- two parameters on a type whose users write `IoRing`
//! today, compounding a break `M28` accepts deliberately with one nobody asked
//! for. Module indirection costs the published API nothing: the signature of
//! every public item is unchanged, and so is the generic slot `M28` is going
//! to need.
//!
//! # What is behind the seam, and what deliberately is not
//!
//! Behind it: `SubmitIoRing`, `PopIoRingCompletion`, and the `Build*` family
//! -- the calls that submit work and report its outcome, which is the surface
//! `M26.1`'s [RESPONSE-SPACE.md](../RESPONSE-SPACE.md) describes.
//!
//! Not behind it: `CreateIoRing`, `CloseIoRing`, `GetIoRingInfo`,
//! `IsIoRingOpSupported`. Those decide whether a ring *exists* and what it
//! supports, not how it responds, and a resolver that replaced them would be
//! a fake ring rather than a resolver over responses. **`M26` does not
//! justify itself on hermeticity** -- the milestone says so in as many words,
//! because `M24` reached a hermetic lib suite without it -- so a resolver runs
//! against a real ring whose operations it answers for. Leaving lifecycle real
//! is what keeps this a seam under the responses rather than a mock of the
//! ring.
//!
//! `SetIoRingCompletionEvent` was moved behind the seam by `M26.3`, and the
//! reason is worth stating because it looks like lifecycle. It is how a
//! completion becomes *observable* to a waiter, so
//! [RS-P-6](../RESPONSE-SPACE.md) -- a completion posted behind another need
//! produce no signal -- is a clause about this call. A resolver that could not
//! make it does not satisfy RS-P-6 vacuously; it never signals at all, which
//! hangs every [`crate::EventDelivery`] consumer rather than testing one.
//!
//! # The trait mirrors the FFI exactly, on purpose
//!
//! Every method takes and returns what the Win32 call takes and returns, with
//! no interpretation. A seam that translated into friendlier types would be
//! encoding a belief about what those calls mean, which is the objection
//! [D-52](../DESIGN-NOTES.md#d-52) records against mocks and the reason this
//! milestone exists. Translating is the *resolver's* job, and it does it
//! against a written specification.

use std::ffi::c_void;

use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingCancelRequest, BuildIoRingFlushFile, BuildIoRingReadFile,
    BuildIoRingRegisterBuffers, BuildIoRingRegisterFileHandles, BuildIoRingWriteFile,
    FILE_FLUSH_MODE, IORING_BUFFER_INFO, IORING_BUFFER_REF, IORING_CQE, IORING_HANDLE_REF,
    PopIoRingCompletion, SetIoRingCompletionEvent, SubmitIoRing,
};
use windows_sys::core::HRESULT;

#[cfg(feature = "kernel-seam")]
mod installed;
#[cfg(feature = "kernel-seam")]
mod resolver;
#[cfg(feature = "kernel-seam")]
pub use installed::{Installed, Responses, install};
#[cfg(feature = "kernel-seam")]
pub use resolver::{
    Resolver, ResolverConfig, ResolverStats, ResolverWatch, SEED_VAR as RESOLVER_SEED_VAR,
};

/// Dispatch to an installed [`Responses`], or fall through to the real call.
///
/// The feature-off arm expands to the real call and nothing else, so a
/// published build has no branch, no thread-local access, and no trait object
/// -- the wrapper is the call.
macro_rules! through_seam {
    ($method:ident ( $($arg:expr),* $(,)? ) else $real:expr) => {{
        #[cfg(feature = "kernel-seam")]
        {
            if let Some(answer) = $crate::sys::installed::with(|r| unsafe { r.$method($($arg),*) })
            {
                return answer;
            }
        }
        unsafe { $real }
    }};
}

/// `SubmitIoRing`.
///
/// # Safety
///
/// `ring` must be a live ring and `submitted` a valid out-pointer, exactly as
/// the Win32 call requires. Forwarded unchanged.
#[inline(always)]
pub(crate) unsafe fn submit(
    ring: *mut c_void,
    wait_operations: u32,
    milliseconds: u32,
    submitted: *mut u32,
) -> HRESULT {
    through_seam!(
        submit(ring, wait_operations, milliseconds, submitted)
            else SubmitIoRing(ring, wait_operations, milliseconds, submitted)
    )
}

/// `PopIoRingCompletion`.
///
/// # Safety
///
/// `ring` must be a live ring and `cqe` a valid out-pointer.
#[inline(always)]
pub(crate) unsafe fn pop(ring: *mut c_void, cqe: *mut IORING_CQE) -> HRESULT {
    through_seam!(pop(ring, cqe) else PopIoRingCompletion(ring, cqe))
}

/// `BuildIoRingReadFile`.
///
/// # Safety
///
/// As the Win32 call: a live ring, a valid handle reference, and a buffer
/// reference that stays valid until the operation completes.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn build_read(
    ring: *mut c_void,
    file: IORING_HANDLE_REF,
    buffer: IORING_BUFFER_REF,
    bytes: u32,
    offset: u64,
    user_data: usize,
    flags: i32,
) -> HRESULT {
    through_seam!(
        build_read(ring, file, buffer, bytes, offset, user_data, flags)
            else BuildIoRingReadFile(ring, file, buffer, bytes, offset, user_data, flags)
    )
}

/// `BuildIoRingWriteFile`.
///
/// # Safety
///
/// As [`build_read`].
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn build_write(
    ring: *mut c_void,
    file: IORING_HANDLE_REF,
    buffer: IORING_BUFFER_REF,
    bytes: u32,
    offset: u64,
    caching: i32,
    user_data: usize,
    flags: i32,
) -> HRESULT {
    through_seam!(
        build_write(ring, file, buffer, bytes, offset, caching, user_data, flags)
            else BuildIoRingWriteFile(ring, file, buffer, bytes, offset, caching, user_data, flags)
    )
}

/// `BuildIoRingFlushFile`.
///
/// # Safety
///
/// `ring` must be live and `file` a valid handle reference.
#[inline(always)]
pub(crate) unsafe fn build_flush(
    ring: *mut c_void,
    file: IORING_HANDLE_REF,
    mode: FILE_FLUSH_MODE,
    user_data: usize,
    flags: i32,
) -> HRESULT {
    through_seam!(
        build_flush(ring, file, mode, user_data, flags)
            else BuildIoRingFlushFile(ring, file, mode, user_data, flags)
    )
}

/// `BuildIoRingCancelRequest`.
///
/// # Safety
///
/// `ring` must be live and `file` a valid handle reference.
#[inline(always)]
pub(crate) unsafe fn build_cancel(
    ring: *mut c_void,
    file: IORING_HANDLE_REF,
    cancel_user_data: usize,
    user_data: usize,
) -> HRESULT {
    through_seam!(
        build_cancel(ring, file, cancel_user_data, user_data)
            else BuildIoRingCancelRequest(ring, file, cancel_user_data, user_data)
    )
}

/// `BuildIoRingRegisterFileHandles`.
///
/// # Safety
///
/// `handles` must point to `count` valid handles that outlive the operation.
#[inline(always)]
pub(crate) unsafe fn build_register_files(
    ring: *mut c_void,
    count: u32,
    handles: *const *mut c_void,
    user_data: usize,
) -> HRESULT {
    through_seam!(
        build_register_files(ring, count, handles, user_data)
            else BuildIoRingRegisterFileHandles(ring, count, handles, user_data)
    )
}

/// `BuildIoRingRegisterBuffers`.
///
/// # Safety
///
/// `buffers` must point to `count` `IORING_BUFFER_INFO` entries that stay
/// valid until the registration operation *runs* -- not merely until this
/// call returns; see [D-32](../DESIGN-NOTES.md#d-32), which measured the
/// difference.
#[inline(always)]
pub(crate) unsafe fn build_register_buffers(
    ring: *mut c_void,
    count: u32,
    buffers: *const IORING_BUFFER_INFO,
    user_data: usize,
) -> HRESULT {
    through_seam!(
        build_register_buffers(ring, count, buffers, user_data)
            else BuildIoRingRegisterBuffers(ring, count, buffers, user_data)
    )
}

/// `SetIoRingCompletionEvent`.
///
/// # Safety
///
/// `ring` must be a live ring and `event` a live event handle the ring will
/// own for the rest of its life.
#[inline(always)]
pub(crate) unsafe fn set_completion_event(ring: *mut c_void, event: *mut c_void) -> HRESULT {
    through_seam!(
        set_completion_event(ring, event) else SetIoRingCompletionEvent(ring, event)
    )
}

/// Re-exported for [`Responses`]' default methods, which forward to the real
/// calls so an implementation overrides only what it varies.
#[cfg(feature = "kernel-seam")]
pub(crate) mod real {
    pub(crate) use windows_sys::Win32::Storage::FileSystem::{
        BuildIoRingCancelRequest, BuildIoRingFlushFile, BuildIoRingReadFile,
        BuildIoRingRegisterBuffers, BuildIoRingRegisterFileHandles, BuildIoRingWriteFile,
        PopIoRingCompletion, SetIoRingCompletionEvent, SubmitIoRing,
    };
}
