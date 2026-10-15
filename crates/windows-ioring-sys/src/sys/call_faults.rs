// Copyright (c) Mike Grier
//! A safe responder that fails a ring's calls, a completion's result, or its submits on demand
//! (M37.1, M38.1).
//!
//! # Why this is here and not in the consumer
//!
//! [`Responses`]' methods are `unsafe fn`, so implementing it needs `unsafe`
//! -- which a consumer such as durable-ioring forbids
//! ([DI-D-4](../../../durable-ioring/DESIGN-NOTES.md#di-d-4)) and which that
//! decision says to answer by adding the safe wrapper to the lower crate. What
//! a consumer's failure paths need is small enough to provide whole: fail a
//! call for as long as, or as many times as, a test says; fail the result of
//! the next completions; and stall submits so an operation stays pending. Every
//! other call reaches the kernel.
//!
//! # Use
//!
//! ```
//! use windows_ioring_sys::sys::{Call, call_faults, register};
//! use windows_ioring_sys::{InjectedFailure, IoRing};
//!
//! let mut ring = IoRing::new(8, 16)?;
//! let (faults, control) = call_faults();
//! // Ended by the ring's drop; nothing here needs it sooner.
//! let _registration = register(&ring, Box::new(faults));
//!
//! // `ERROR_IO_DEVICE`, once: the first attach of the completion event fails.
//! control.fail_next(Call::SetCompletionEvent, 1, InjectedFailure::Win32(1117));
//! assert!(ring.completion_event().is_err());
//! assert!(ring.completion_event().is_ok(), "and the next reaches the kernel");
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! [`submit_faults`] is the narrow form for the one call a consumer most
//! often wants failed:
//!
//! ```
//! use windows_ioring_sys::sys::{register, submit_faults};
//! use windows_ioring_sys::{Batch, InjectedFailure, IoRing};
//!
//! let mut ring = IoRing::new(8, 16)?;
//! let (faults, control) = submit_faults();
//! let _registration = register(&ring, Box::new(faults));
//!
//! // `ERROR_IO_DEVICE`: the ring's submits, on any thread, now fail.
//! control.fail_until_cleared(InjectedFailure::Win32(1117));
//! assert!(Batch::new(&mut ring).submit().is_err());
//!
//! control.clear();
//! Batch::new(&mut ring).submit()?;
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # What a failed call is
//!
//! It returns the failure **without reaching the kernel**. For a submit that
//! means every entry it was asked to submit stays in the submission queue,
//! which is what `SubmitIoRing` documents for a failure (RS-P-7): a later
//! submit is what runs them. A call's out-parameters are left untouched,
//! because what the real call writes there on failure is undocumented and
//! writing anything would invent a platform.
//!
//! **A submit that waits fails too.** A ring dropped with the fault still
//! armed and operations outstanding cannot run down -- its rundown is a
//! submit -- so clear the fault first.
//!
//! # What a failed completion is
//!
//! [`fail_next_completions`](CallFaultControl::fail_next_completions) pops the
//! real completion and rewrites only its result code, the identity and
//! everything else untouched -- what
//! [`Completion::with_injected_failure`](crate::Completion::with_injected_failure)
//! does to a popped one, here on the way out of the seam, so a consumer that
//! never sees the raw completion can still be failed.
//!
//! # What a stalled submit is
//!
//! [`stall_submits`](CallFaultControl::stall_submits) leaves an operation
//! pending, which the platform permits (RS-P-1). A stalled submit returns
//! without reaching the kernel: success with nothing submitted if it does not
//! wait, and `IORING_E_WAIT_TIMEOUT` after its wait -- or what is left of the
//! stall, if less -- if it does, which is the documented answer to a wait that
//! expired. So a bounded wait for the operation's completion expires, and a
//! ring dropped meanwhile still runs down once the stall ends. The stall runs
//! from the **first submit it stalled**, not from when it was armed, so
//! nothing a test does between arming and submitting shortens it.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::S_OK;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLUSH_MODE, IORING_BUFFER_INFO, IORING_BUFFER_REF, IORING_CQE, IORING_HANDLE_REF,
};
use windows_sys::core::HRESULT;

use super::Responses;
use crate::InjectedFailure;
use crate::ring::IORING_E_WAIT_TIMEOUT;

/// A call the seam carries, which [`CallFaults`] can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Call {
    /// `SubmitIoRing`.
    Submit,
    /// `PopIoRingCompletion`, the call itself failing. A completion's result
    /// failing is [`CallFaultControl::fail_next_completions`].
    Pop,
    /// `BuildIoRingReadFile`.
    BuildRead,
    /// `BuildIoRingWriteFile`.
    BuildWrite,
    /// `BuildIoRingFlushFile`.
    BuildFlush,
    /// `BuildIoRingCancelRequest`.
    BuildCancel,
    /// `BuildIoRingRegisterFileHandles`.
    BuildRegisterFiles,
    /// `BuildIoRingRegisterBuffers`.
    BuildRegisterBuffers,
    /// `SetIoRingCompletionEvent`.
    SetCompletionEvent,
}

/// A failure armed: the code, and how many more times, or until cleared when `None`.
#[derive(Debug, Clone, Copy)]
struct Fail {
    code: HRESULT,
    remaining: Option<u32>,
}

impl Fail {
    /// Spend one use of the failure in `slot`: its code, and what is left of it.
    fn spend(slot: &mut Option<Self>) -> Option<HRESULT> {
        let fail = (*slot)?;
        *slot = match fail.remaining {
            Some(left) if left > 1 => Some(Self {
                remaining: Some(left - 1),
                ..fail
            }),
            Some(_) => None,
            None => Some(fail),
        };
        Some(fail.code)
    }
}

/// Whether submits are stalled.
#[derive(Debug, Clone, Copy)]
enum Stall {
    Off,
    /// Armed for this long, not yet begun: it begins at the first submit it stalls.
    For(Duration),
    /// Begun at this instant, for this long.
    Since(Instant, Duration),
}

#[derive(Debug)]
struct State {
    calls: HashMap<Call, Fail>,
    completions: Option<Fail>,
    stall: Stall,
    /// The call and thread of each failed call, in order.
    failed_from: Vec<(Call, ThreadId)>,
}

/// The state shared by a responder and its control, under one lock so a
/// failure is recorded in the same step that decides it.
type Shared = Arc<Mutex<State>>;

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reaches the kernel for every call: the trait's defaults.
struct Forward;

impl Responses for Forward {}

/// A responder that fails its ring's calls, completions and submits when armed through its
/// [`CallFaultControl`], and forwards everything else to the kernel.
///
/// Register it with [`register`](super::register).
#[derive(Debug)]
pub struct CallFaults {
    shared: Shared,
}

/// Arms and reads a [`CallFaults`] from any thread.
#[derive(Debug, Clone)]
pub struct CallFaultControl {
    shared: Shared,
}

/// A responder that fails a ring's calls on demand, and the handle that arms it. Nothing fails
/// until armed.
#[must_use]
pub fn call_faults() -> (CallFaults, CallFaultControl) {
    let shared = Arc::new(Mutex::new(State {
        calls: HashMap::new(),
        completions: None,
        stall: Stall::Off,
        failed_from: Vec::new(),
    }));
    (
        CallFaults {
            shared: Arc::clone(&shared),
        },
        CallFaultControl { shared },
    )
}

impl CallFaultControl {
    /// Fail the next `count` calls of `call` with `failure`, then reach the kernel again. A
    /// `count` of zero clears `call`.
    ///
    /// # Panics
    ///
    /// If `failure` is not a failing `HRESULT`, which only [`InjectedFailure::Hresult`] can
    /// express: this seam injects failure, never success.
    pub fn fail_next(&self, call: Call, count: u32, failure: InjectedFailure) {
        let code = failure.as_hresult();
        let mut state = lock(&self.shared);
        if count == 0 {
            state.calls.remove(&call);
        } else {
            state.calls.insert(
                call,
                Fail {
                    code,
                    remaining: Some(count),
                },
            );
        }
    }

    /// Fail every call of `call` with `failure` until [`clear`](Self::clear).
    ///
    /// # Panics
    ///
    /// As [`fail_next`](Self::fail_next).
    pub fn fail_until_cleared(&self, call: Call, failure: InjectedFailure) {
        let code = failure.as_hresult();
        lock(&self.shared).calls.insert(
            call,
            Fail {
                code,
                remaining: None,
            },
        );
    }

    /// Rewrite the result code of the next `count` completions popped to `failure`, the identity
    /// and everything else in them untouched. A `count` of zero clears.
    ///
    /// # Panics
    ///
    /// As [`fail_next`](Self::fail_next).
    pub fn fail_next_completions(&self, count: u32, failure: InjectedFailure) {
        let code = failure.as_hresult();
        lock(&self.shared).completions = (count != 0).then_some(Fail {
            code,
            remaining: Some(count),
        });
    }

    /// Stall submits for `duration`, from the first submit stalled: each returns without reaching
    /// the kernel, so what is queued stays pending, until `duration` has passed and submits reach
    /// the kernel and run it. A `duration` of zero clears.
    pub fn stall_submits(&self, duration: Duration) {
        lock(&self.shared).stall = if duration.is_zero() {
            Stall::Off
        } else {
            Stall::For(duration)
        };
    }

    /// Stop failing and stalling: every call reaches the kernel again. What was recorded is kept.
    pub fn clear(&self) {
        let mut state = lock(&self.shared);
        state.calls.clear();
        state.completions = None;
        state.stall = Stall::Off;
    }

    /// The thread that made each failed call of `call` so far, in order.
    ///
    /// A test that arms a failure on one thread and wants to show it was answered on another -- a
    /// pool thread draining the ring -- compares these with its own.
    #[must_use]
    pub fn failures(&self, call: Call) -> Vec<ThreadId> {
        lock(&self.shared)
            .failed_from
            .iter()
            .filter(|(failed, _)| *failed == call)
            .map(|(_, thread)| *thread)
            .collect()
    }
}

impl CallFaults {
    /// The failure armed for `call`, spent, if one is.
    fn fails(&self, call: Call) -> Option<HRESULT> {
        let mut state = lock(&self.shared);
        let mut slot = state.calls.get(&call).copied();
        let code = Fail::spend(&mut slot)?;
        match slot {
            Some(armed) => state.calls.insert(call, armed),
            None => state.calls.remove(&call),
        };
        state.failed_from.push((call, std::thread::current().id()));
        Some(code)
    }

    /// How a stalled submit answers, if submits are stalled and the stall has not ended: the
    /// answer, and how long to wait before giving it.
    fn stalled(&self, wait_operations: u32, milliseconds: u32) -> Option<(HRESULT, Duration)> {
        let mut state = lock(&self.shared);
        let (since, length) = match state.stall {
            Stall::Off => return None,
            Stall::For(length) => {
                let now = Instant::now();
                state.stall = Stall::Since(now, length);
                (now, length)
            }
            Stall::Since(since, length) => (since, length),
        };
        let left = length.saturating_sub(since.elapsed());
        if left.is_zero() {
            state.stall = Stall::Off;
            return None;
        }
        Some(if wait_operations == 0 {
            (S_OK, Duration::ZERO)
        } else {
            (
                IORING_E_WAIT_TIMEOUT,
                left.min(Duration::from_millis(u64::from(milliseconds))),
            )
        })
    }
}

impl Responses for CallFaults {
    unsafe fn submit(
        &mut self,
        ring: *mut c_void,
        wait_operations: u32,
        milliseconds: u32,
        submitted: *mut u32,
    ) -> HRESULT {
        if let Some(code) = self.fails(Call::Submit) {
            return code;
        }
        if let Some((answer, wait)) = self.stalled(wait_operations, milliseconds) {
            std::thread::sleep(wait);
            if answer == S_OK && !submitted.is_null() {
                // SAFETY: `submitted` is the caller's valid out-pointer, as `SubmitIoRing`
                // requires; nothing was submitted, which is what is written.
                unsafe { submitted.write(0) };
            }
            return answer;
        }
        // SAFETY: forwarded unchanged from the seam's caller, who holds the Win32 call's own
        // contract.
        unsafe { Forward.submit(ring, wait_operations, milliseconds, submitted) }
    }

    unsafe fn pop(&mut self, ring: *mut c_void, cqe: *mut IORING_CQE) -> HRESULT {
        if let Some(code) = self.fails(Call::Pop) {
            return code;
        }
        // SAFETY: as `submit`.
        let answer = unsafe { Forward.pop(ring, cqe) };
        // `S_OK` is a completion popped; an empty queue answers `S_FALSE`, with nothing to fail.
        if answer == S_OK
            && let Some(code) = Fail::spend(&mut lock(&self.shared).completions)
        {
            // SAFETY: `cqe` is the caller's valid out-pointer, and the real pop just wrote it.
            unsafe { (*cqe).ResultCode = code };
        }
        answer
    }

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
        if let Some(code) = self.fails(Call::BuildRead) {
            return code;
        }
        // SAFETY: as `submit`.
        unsafe { Forward.build_read(ring, file, buffer, bytes, offset, user_data, flags) }
    }

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
        if let Some(code) = self.fails(Call::BuildWrite) {
            return code;
        }
        // SAFETY: as `submit`.
        unsafe { Forward.build_write(ring, file, buffer, bytes, offset, caching, user_data, flags) }
    }

    unsafe fn build_flush(
        &mut self,
        ring: *mut c_void,
        file: IORING_HANDLE_REF,
        mode: FILE_FLUSH_MODE,
        user_data: usize,
        flags: i32,
    ) -> HRESULT {
        if let Some(code) = self.fails(Call::BuildFlush) {
            return code;
        }
        // SAFETY: as `submit`.
        unsafe { Forward.build_flush(ring, file, mode, user_data, flags) }
    }

    unsafe fn build_cancel(
        &mut self,
        ring: *mut c_void,
        file: IORING_HANDLE_REF,
        cancel_user_data: usize,
        user_data: usize,
    ) -> HRESULT {
        if let Some(code) = self.fails(Call::BuildCancel) {
            return code;
        }
        // SAFETY: as `submit`.
        unsafe { Forward.build_cancel(ring, file, cancel_user_data, user_data) }
    }

    unsafe fn build_register_files(
        &mut self,
        ring: *mut c_void,
        count: u32,
        handles: *const *mut c_void,
        user_data: usize,
    ) -> HRESULT {
        if let Some(code) = self.fails(Call::BuildRegisterFiles) {
            return code;
        }
        // SAFETY: as `submit`.
        unsafe { Forward.build_register_files(ring, count, handles, user_data) }
    }

    unsafe fn build_register_buffers(
        &mut self,
        ring: *mut c_void,
        count: u32,
        buffers: *const IORING_BUFFER_INFO,
        user_data: usize,
    ) -> HRESULT {
        if let Some(code) = self.fails(Call::BuildRegisterBuffers) {
            return code;
        }
        // SAFETY: as `submit`.
        unsafe { Forward.build_register_buffers(ring, count, buffers, user_data) }
    }

    unsafe fn set_completion_event(&mut self, ring: *mut c_void, event: *mut c_void) -> HRESULT {
        if let Some(code) = self.fails(Call::SetCompletionEvent) {
            return code;
        }
        // SAFETY: as `submit`.
        unsafe { Forward.set_completion_event(ring, event) }
    }
}

/// A responder that fails a ring's submits on demand, and the narrow handle that arms it:
/// [`call_faults`], with the control reduced to [`Call::Submit`]. Nothing fails until armed.
#[must_use]
pub fn submit_faults() -> (CallFaults, SubmitFaultControl) {
    let (faults, control) = call_faults();
    (faults, SubmitFaultControl { control })
}

/// Arms and reads a ring's submit failures from any thread: a [`CallFaultControl`] that can only
/// fail [`Call::Submit`].
#[derive(Debug, Clone)]
pub struct SubmitFaultControl {
    control: CallFaultControl,
}

impl SubmitFaultControl {
    /// Fail the next `count` submits with `failure`, then reach the kernel again. A `count` of
    /// zero clears.
    ///
    /// # Panics
    ///
    /// If `failure` is not a failing `HRESULT`, which only [`InjectedFailure::Hresult`] can
    /// express: this seam injects failure, never success.
    pub fn fail_next(&self, count: u32, failure: InjectedFailure) {
        self.control.fail_next(Call::Submit, count, failure);
    }

    /// Fail every submit with `failure` until [`clear`](Self::clear).
    ///
    /// # Panics
    ///
    /// As [`fail_next`](Self::fail_next).
    pub fn fail_until_cleared(&self, failure: InjectedFailure) {
        self.control.fail_until_cleared(Call::Submit, failure);
    }

    /// Stop failing: submits reach the kernel again.
    pub fn clear(&self) {
        self.control.clear();
    }

    /// The thread that made each failed submit so far, in order.
    #[must_use]
    pub fn failures(&self) -> Vec<ThreadId> {
        self.control.failures(Call::Submit)
    }
}

#[cfg(test)]
mod tests;
