// Copyright (c) Mike Grier
//! Installing a [`Responses`] under the seam (M26.2).
//!
//! # Thread-local, and that is not a detail
//!
//! `cargo test` runs tests as **threads in one process**, which this
//! repository chose deliberately and documents in its design notes. A
//! process-global responder would therefore let one test answer another
//! test's kernel calls, and the failure would look like a flaky ring rather
//! than like a harness defect. The crate already reached this conclusion once,
//! for `IoRing`'s drop counter, after a process-wide static let another test's
//! drop satisfy an assertion and mask the mutation the test existed to catch.
//!
//! So the responder is installed per thread, and a ring used from a thread
//! with nothing installed talks to the real kernel -- including a ring moved
//! across threads, which is legal since `IoRing` is `Send`. A resolver that
//! needs to follow a ring across threads has to arrange it; nothing here does
//! it implicitly.

use std::cell::RefCell;
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
    /// As `BuildIoRingRegisterFileHandles`.
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
    /// which is what [RS-P-6](../RESPONSE-SPACE.md) is a clause about -- not
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

thread_local! {
    /// The responder for this thread, if one is installed.
    ///
    /// `RefCell` rather than `Cell`: the seam hands out `&mut` so a responder
    /// can keep state across calls, which any resolver over an ordering space
    /// must. The borrow is held only for the duration of one FFI call.
    static CURRENT: RefCell<Option<Box<dyn Responses>>> = const { RefCell::new(None) };
}

/// Run `f` against this thread's responder, if it has one.
///
/// `None` means nothing is installed, which is the seam's signal to make the
/// real call.
///
/// **A responder is not consulted re-entrantly.** If `f` reaches the seam
/// again -- a responder that forwards to the real call cannot, but one driving
/// a nested ring could -- the inner call finds the slot already borrowed and
/// falls through to the kernel rather than panicking on the `RefCell` or
/// aliasing the `&mut`.
pub(crate) fn with<R>(f: impl FnOnce(&mut dyn Responses) -> R) -> Option<R> {
    CURRENT
        .try_with(|slot| {
            let Ok(mut borrow) = slot.try_borrow_mut() else {
                return None;
            };
            borrow.as_mut().map(|responder| f(&mut **responder))
        })
        .ok()
        .flatten()
}

/// Installs `responder` for this thread until the returned guard drops.
///
/// # Panics
///
/// If this thread already has one installed. Nesting would make which
/// responder answered a given call depend on drop order, and a suite that
/// nested by accident would be very hard to read.
#[must_use = "the responder is uninstalled when the guard drops"]
pub fn install(responder: Box<dyn Responses>) -> Installed {
    CURRENT.with(|slot| {
        let mut borrow = slot.borrow_mut();
        assert!(
            borrow.is_none(),
            "a kernel responder is already installed on this thread"
        );
        *borrow = Some(responder);
    });
    Installed { _private: () }
}

/// Uninstalls this thread's responder on drop.
#[must_use = "dropping this immediately uninstalls the responder"]
pub struct Installed {
    _private: (),
}

impl Drop for Installed {
    fn drop(&mut self) {
        // `try_with` rather than `with`: on a thread being torn down the
        // thread-local may already be destroyed, and a panic in `Drop` during
        // that unwind would abort the process (M23.4).
        let _ = CURRENT.try_with(|slot| {
            if let Ok(mut borrow) = slot.try_borrow_mut() {
                *borrow = None;
            }
        });
    }
}

#[cfg(test)]
mod tests;
