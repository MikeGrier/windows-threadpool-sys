// Copyright (c) Mike Grier.

//! Capture a Windows thread's ambient state and apply it on another thread.
//!
//! Some Windows behaviour is not a parameter of the call you make; it is
//! ambient state hanging off the calling thread. An impersonation token decides
//! whose access rights an open is checked against, and even which drive letters
//! resolve. The thread error mode decides whether a hard device error raises a
//! modal dialog. WOW64 filesystem redirection decides which of two directories a
//! 32-bit process actually reaches. None of it travels with work handed to
//! another thread.
//!
//! That matters most when the other thread is shared. A thread-pool worker
//! inherits none of the submitter's ambient state: measured, `OpenThreadToken`
//! on a worker returns `ERROR_NO_TOKEN` while the submitting thread genuinely
//! held a token, and the worker's error mode is `0`, meaning the critical-error
//! handler is enabled and an absent removable drive can put a modal dialog on
//! process-shared infrastructure. Explicit capture is therefore necessary rather
//! than merely prudent.
//!
//! # Scope
//!
//! This crate carries thread-scoped ambient state that changes what a Win32 call
//! does. It does not carry call parameters, does not open files, and does not
//! know what any particular Windows operation is.
//!
//! # Two sets, because the aspects do not relate to the caller the same way
//!
//! Aspects that can be read off the calling thread are **captured**, and which
//! of them to collect is chosen by the caller. Aspects that cannot be read --
//! WOW64 redirection has no getter at all, and I/O priority has no documented
//! one -- are **declared** instead: the caller states the value it wants
//! installed. A declared aspect has nothing to collect, so it is not part of the
//! capture set; left unspecified, it leaves the target thread's own value alone.
//!
//! # This crate holds no policy
//!
//! Every aspect is offered for capture *and* for explicit declaration, and no
//! combination is privileged. A consumer running on shared threads will want to
//! force the dialog-suppressing error-mode bits; a consumer with a private
//! thread, where a modal dialog is its own problem and nobody else's, is
//! entitled to the opposite choice. Both compose that policy from the primitives
//! here rather than finding it already decided.
//!
//! # Example
//!
//! Capture on the submitting thread, where a failure is still the caller's to
//! see, then reconstruct the context on a worker that inherited none of it:
//!
//! ```
//! use std::thread;
//!
//! use windows_thread_ambient_sys::declared::MemoryPriority;
//! use windows_thread_ambient_sys::{AmbientState, CaptureSet, Declared};
//!
//! // Captured here, on the submitting thread, where a failure is still ours to
//! // see. Declared aspects are stated rather than read from anything.
//! let state = AmbientState::capture(CaptureSet::DEFAULT)?
//!     .with_declared(Declared::none().with_memory_priority(MemoryPriority::Low));
//!
//! let applied = thread::spawn(move || {
//!     // Guards apply outermost-first and release in exact reverse, with
//!     // impersonation innermost because its window is the narrowest.
//!     state.with_applied(|| "ran as the submitter")
//! })
//! .join()
//! .expect("the worker did not panic")?;
//!
//! assert_eq!(*applied.value(), "ran as the submitter");
//!
//! // A restoration failure does not discard the operation's value; it is
//! // reported alongside it, so a caller can retire a contaminated thread
//! // without losing what the work produced.
//! assert!(applied.restore().is_clean());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! A consumer that wants to *override* the error mode rather than transplant it
//! -- forcing the dialog-suppressing bits on a shared worker -- leaves
//! [`CaptureSet::ERROR_MODE`] out of its capture set and wraps the call in its
//! own [`ThreadErrorMode::apply`] guard, which then sits outermost, exactly
//! where the ordering puts it.

#![cfg(windows)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod capture_set;
pub mod captured;
pub mod declared;
pub mod error_mode;
pub mod impersonation;
pub mod state;
pub mod transaction;

pub use capture_set::{CapturableAspect, CaptureSet};
pub use captured::Captured;
pub use declared::Declared;
pub use state::{
    AmbientState, Applied, ApplyError, ApplyFailure, CaptureError, CaptureFailure, RestoreReport,
};

/// Compiles the README's examples, so a contract change breaks the build rather
/// than silently teaching the old answer.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
pub use error_mode::ThreadErrorMode;
pub use windows_impersonation_token_sys::ImpersonationToken;
