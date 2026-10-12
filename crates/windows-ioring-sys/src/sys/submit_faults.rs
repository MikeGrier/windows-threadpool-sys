// Copyright (c) Mike Grier
//! A safe responder that fails a ring's submits on demand (M37.1).
//!
//! # Why this is here and not in the consumer
//!
//! [`Responses`]' methods are `unsafe fn`, so implementing it needs `unsafe`
//! -- which a consumer such as durable-ioring forbids
//! ([DI-D-4](../../../durable-ioring/DESIGN-NOTES.md#di-d-4)) and which that
//! decision says to answer by adding the safe wrapper to the lower crate. The
//! common case is small enough to provide whole: fail a ring's submits for as
//! long as, or as many times as, a test says, and let every other call reach
//! the kernel.
//!
//! # Use
//!
//! ```
//! use windows_ioring_sys::sys::{register, submit_faults};
//! use windows_ioring_sys::{Batch, InjectedFailure, IoRing};
//!
//! let mut ring = IoRing::new(8, 16)?;
//! let (faults, control) = submit_faults();
//! // Ended by the ring's drop; nothing here needs it sooner.
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
//! # What a failed submit is
//!
//! It returns the failure **without reaching the kernel**, so every entry it
//! was asked to submit stays in the submission queue, which is what
//! `SubmitIoRing` documents for a failure (RS-P-7): a later submit is what
//! runs them. Its out-parameter is left untouched, because what the real call
//! writes there on failure is undocumented and writing anything would invent
//! a platform.
//!
//! **A submit that waits fails too.** A ring dropped with the fault still
//! armed and operations outstanding cannot run down -- its rundown is a
//! submit -- so clear the fault first.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::ThreadId;

use windows_sys::core::HRESULT;

use super::Responses;
use super::real;
use crate::InjectedFailure;

/// What the next submits do.
#[derive(Debug, Clone, Copy)]
enum Plan {
    /// Reach the kernel.
    Pass,
    /// Fail with `code`: `remaining` more times, or until cleared when `None`.
    Fail {
        code: HRESULT,
        remaining: Option<u32>,
    },
}

#[derive(Debug)]
struct State {
    plan: Plan,
    /// The thread that made each failed submit, in order.
    failed_from: Vec<ThreadId>,
}

/// The state shared by a responder and its control, under one lock so a
/// failure is recorded in the same step that decides it.
type Shared = Arc<Mutex<State>>;

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A responder that fails its ring's submits when armed through its
/// [`SubmitFaultControl`], and forwards every other call to the kernel.
///
/// Register it with [`register`](super::register).
#[derive(Debug)]
pub struct SubmitFaults {
    shared: Shared,
}

/// Arms and reads a [`SubmitFaults`] from any thread.
#[derive(Debug, Clone)]
pub struct SubmitFaultControl {
    shared: Shared,
}

/// A responder that fails a ring's submits on demand, and the handle that arms
/// it. Nothing fails until armed.
#[must_use]
pub fn submit_faults() -> (SubmitFaults, SubmitFaultControl) {
    let shared = Arc::new(Mutex::new(State {
        plan: Plan::Pass,
        failed_from: Vec::new(),
    }));
    (
        SubmitFaults {
            shared: Arc::clone(&shared),
        },
        SubmitFaultControl { shared },
    )
}

impl SubmitFaultControl {
    /// Fail the next `count` submits with `failure`, then reach the kernel
    /// again. A `count` of zero clears.
    ///
    /// # Panics
    ///
    /// If `failure` is not a failing `HRESULT`, which only
    /// [`InjectedFailure::Hresult`] can express: this seam injects failure,
    /// never success.
    pub fn fail_next(&self, count: u32, failure: InjectedFailure) {
        let code = failure.as_hresult();
        lock(&self.shared).plan = if count == 0 {
            Plan::Pass
        } else {
            Plan::Fail {
                code,
                remaining: Some(count),
            }
        };
    }

    /// Fail every submit with `failure` until [`clear`](Self::clear).
    ///
    /// # Panics
    ///
    /// As [`fail_next`](Self::fail_next).
    pub fn fail_until_cleared(&self, failure: InjectedFailure) {
        let code = failure.as_hresult();
        lock(&self.shared).plan = Plan::Fail {
            code,
            remaining: None,
        };
    }

    /// Stop failing: submits reach the kernel again.
    pub fn clear(&self) {
        lock(&self.shared).plan = Plan::Pass;
    }

    /// The thread that made each failed submit so far, in order.
    ///
    /// A test that arms the fault on one thread and wants to show it was
    /// answered on another -- a pool thread draining the ring -- compares
    /// these with its own.
    #[must_use]
    pub fn failures(&self) -> Vec<ThreadId> {
        lock(&self.shared).failed_from.clone()
    }
}

impl Responses for SubmitFaults {
    unsafe fn submit(
        &mut self,
        ring: *mut c_void,
        wait_operations: u32,
        milliseconds: u32,
        submitted: *mut u32,
    ) -> HRESULT {
        {
            let mut state = lock(&self.shared);
            if let Plan::Fail { code, remaining } = state.plan {
                state.plan = match remaining {
                    Some(left) if left > 1 => Plan::Fail {
                        code,
                        remaining: Some(left - 1),
                    },
                    Some(_) => Plan::Pass,
                    None => Plan::Fail {
                        code,
                        remaining: None,
                    },
                };
                state.failed_from.push(std::thread::current().id());
                return code;
            }
        }
        // SAFETY: forwarded unchanged from the seam's caller, who holds the
        // Win32 call's own contract.
        unsafe { real::SubmitIoRing(ring, wait_operations, milliseconds, submitted) }
    }
}

#[cfg(test)]
mod tests;
