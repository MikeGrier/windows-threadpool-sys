// Copyright (c) Mike Grier
//! The [`Responses`] trait: what answers a ring's calls under the seam
//! (M26.2), registered for one ring by [`super::register`]
//! ([D-85](../../DESIGN-NOTES.md#d-85)).
//!
//! It was installed per thread until `M35.2`. That isolated tests running as
//! threads of one process from each other's calls, but a ring used from more
//! than one thread was answered on only one of them, and silently by the
//! kernel on the rest; a ring is now the unit, which isolates tests as well,
//! since two tests do not share one.

use std::ffi::c_void;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLUSH_MODE, IORING_BUFFER_INFO, IORING_BUFFER_REF, IORING_CQE, IORING_HANDLE_REF,
};
use windows_sys::core::HRESULT;

use super::real;

/// Answers the `IoRing` calls that carry an operation.
///
/// Every method mirrors its Win32 call exactly and **defaults to making it**,
/// so an implementation overrides only the calls whose responses it varies.
/// That default is what keeps a partial resolver honest: a method left
/// unimplemented behaves like the kernel rather than like a stub returning
/// success.
///
/// # Safety
///
/// Every method is `unsafe` because every method may be forwarded to the Win32
/// call with the caller's pointers. An implementation that does not forward
/// still receives raw pointers and must not dereference them beyond what the
/// corresponding call would.
pub trait Responses {
    /// `SubmitIoRing`.
    ///
    /// # Safety
    ///
    /// As `SubmitIoRing`.
    unsafe fn submit(
        &mut self,
        ring: *mut c_void,
        wait_operations: u32,
        milliseconds: u32,
        submitted: *mut u32,
    ) -> HRESULT {
        // SAFETY: forwarded unchanged from the caller, who holds the Win32
        // call's own contract.
        unsafe { real::SubmitIoRing(ring, wait_operations, milliseconds, submitted) }
    }

    /// `PopIoRingCompletion`.
    ///
    /// A completion this writes must carry an identity the ring minted and has
    /// not yet seen complete (`RS-C-2`). Any other `UserData` is reported by
    /// the ring as a defect, and the pop that receives it panics
    /// ([D-79](../../DESIGN-NOTES.md#d-79)).
    ///
    /// # Safety
    ///
    /// As `PopIoRingCompletion`.
    unsafe fn pop(&mut self, ring: *mut c_void, cqe: *mut IORING_CQE) -> HRESULT {
        // SAFETY: as `submit`.
        unsafe { real::PopIoRingCompletion(ring, cqe) }
    }

    /// `BuildIoRingReadFile`.
    ///
    /// # Safety
    ///
    /// As `BuildIoRingReadFile`.
    #[allow(clippy::too_many_arguments)]
    unsafe fn build_read(
        &mut self,
        ring: *mut c_void,
        file: IORING_HANDLE_REF,
        buffer: IORING_BUFFER_REF,
        bytes: u32,
        offset: u64,
        user_data: usize,
        flags: i32,
    ) -> HRESULT {
        // SAFETY: as `submit`.
        unsafe { real::BuildIoRingReadFile(ring, file, buffer, bytes, offset, user_data, flags) }
    }

    /// `BuildIoRingWriteFile`.
    ///
    /// # Safety
    ///
    /// As `BuildIoRingWriteFile`.
    #[allow(clippy::too_many_arguments)]
    unsafe fn build_write(
        &mut self,
        ring: *mut c_void,
        file: IORING_HANDLE_REF,
        buffer: IORING_BUFFER_REF,
        bytes: u32,
        offset: u64,
        caching: i32,
        user_data: usize,
        flags: i32,
    ) -> HRESULT {
        // SAFETY: as `submit`.
        unsafe {
            real::BuildIoRingWriteFile(ring, file, buffer, bytes, offset, caching, user_data, flags)
        }
    }

    /// `BuildIoRingFlushFile`.
    ///
    /// # Safety
    ///
    /// As `BuildIoRingFlushFile`.
    unsafe fn build_flush(
        &mut self,
        ring: *mut c_void,
        file: IORING_HANDLE_REF,
        mode: FILE_FLUSH_MODE,
        user_data: usize,
        flags: i32,
    ) -> HRESULT {
        // SAFETY: as `submit`.
        unsafe { real::BuildIoRingFlushFile(ring, file, mode, user_data, flags) }
    }

    /// `BuildIoRingCancelRequest`.
    ///
    /// # Safety
    ///
    /// As `BuildIoRingCancelRequest`.
    unsafe fn build_cancel(
        &mut self,
        ring: *mut c_void,
        file: IORING_HANDLE_REF,
        cancel_user_data: usize,
        user_data: usize,
    ) -> HRESULT {
        // SAFETY: as `submit`.
        unsafe { real::BuildIoRingCancelRequest(ring, file, cancel_user_data, user_data) }
    }

    /// `BuildIoRingRegisterFileHandles`.
    ///
    /// # Safety
    ///
    /// As `BuildIoRingRegisterFileHandles`, including that `handles` stays
    /// valid until the registration operation *runs* (`D-32`).
    unsafe fn build_register_files(
        &mut self,
        ring: *mut c_void,
        count: u32,
        handles: *const *mut c_void,
        user_data: usize,
    ) -> HRESULT {
        // SAFETY: as `submit`.
        unsafe { real::BuildIoRingRegisterFileHandles(ring, count, handles, user_data) }
    }

    /// `BuildIoRingRegisterBuffers`.
    ///
    /// # Safety
    ///
    /// As `BuildIoRingRegisterBuffers`, including that `buffers` stays valid
    /// until the registration operation *runs* rather than merely until the
    /// call returns -- a difference this crate measured (`D-32`).
    unsafe fn build_register_buffers(
        &mut self,
        ring: *mut c_void,
        count: u32,
        buffers: *const IORING_BUFFER_INFO,
        user_data: usize,
    ) -> HRESULT {
        // SAFETY: as `submit`.
        unsafe { real::BuildIoRingRegisterBuffers(ring, count, buffers, user_data) }
    }

    /// `SetIoRingCompletionEvent`.
    ///
    /// Behind the seam because it is how a completion becomes *observable*,
    /// which is what [RS-P-6](../../RESPONSE-SPACE.md) is a clause about -- not
    /// because it is lifecycle. An implementation that answers this call
    /// takes on the obligation to signal, since a ring whose completions are
    /// answered here and whose event is never set leaves every waiter parked.
    ///
    /// # Safety
    ///
    /// As `SetIoRingCompletionEvent`. An implementation that keeps `event`
    /// must not outlive the ring that owns it.
    unsafe fn set_completion_event(&mut self, ring: *mut c_void, event: *mut c_void) -> HRESULT {
        // SAFETY: as `submit`.
        unsafe { real::SetIoRingCompletionEvent(ring, event) }
    }
}
