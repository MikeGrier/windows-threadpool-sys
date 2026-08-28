// Copyright (c) Mike Grier.

//! The impersonation aspect.
//!
//! A thread-pool worker inherits no impersonation token: measured,
//! `OpenThreadToken` on a worker returns `ERROR_NO_TOKEN` while the submitting
//! thread genuinely held one. So work remoted onto a worker is access-checked as
//! the *process* unless the caller's context is carried explicitly.
//!
//! # This module does not implement impersonation
//!
//! Capture, transport, thread-bound application, and exact restoration are owned
//! by [`windows_impersonation_token_sys`], which is independently published and
//! is the platform layer for this one Windows concept. This module adapts it to
//! the crate's three-state [`Captured`] shape and to subset application; it does
//! not reimplement any part of it, and it does not soften its semantics.
//!
//! In particular, **restore failure remains fail-fast**, inherited rather than
//! chosen. Returning a shared worker to a pool under an unknown identity is a
//! process-wide security failure, which is a different order of hazard from the
//! other aspects, and the reason this crate composes per-aspect guards instead
//! of one guard with one policy.
//!
//! # Why `Absent` is unreachable here
//!
//! [`ImpersonationToken::capture`] never reports "the thread had no token": when
//! the calling thread is not impersonating it snapshots the process identity as
//! a `SecurityImpersonation` token. So [`Captured::Absent`] cannot occur for
//! this aspect. The three-state shape is kept anyway, because a per-aspect shape
//! would oblige every consumer to remember which aspects can be absent -- see
//! [`Captured`].
//!
//! # Example
//!
//! ```
//! use std::thread;
//!
//! use windows_thread_ambient_sys::impersonation;
//!
//! // Capture on the submitting thread: a failure is reported where the caller
//! // can still act on it, rather than arriving later from a worker.
//! let context = impersonation::capture()?;
//!
//! let value = thread::spawn(move || {
//!     // A fresh worker inherits no token, so the context has to be reapplied.
//!     impersonation::with_applied(&context, || "checked as the submitter")
//! })
//! .join()
//! .expect("the worker did not panic")?;
//!
//! assert_eq!(value, "checked as the submitter");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use windows_impersonation_token_sys::{ApplyError, CaptureError, ImpersonationToken};

use crate::captured::Captured;

/// Capture the calling thread's impersonation context.
///
/// Capture is synchronous and happens on the caller's own thread, so a failure
/// is reported where the caller can still do something about it rather than
/// arriving later from a worker.
///
/// The result is always [`Captured::Present`] on success; see the module
/// documentation for why [`Captured::Absent`] is unreachable.
///
/// # Errors
///
/// Returns [`CaptureError`] if the context cannot be captured -- notably for an
/// anonymous impersonation context, which Windows does not permit to be opened.
pub fn capture() -> Result<Captured<ImpersonationToken>, CaptureError> {
    ImpersonationToken::capture().map(Captured::Present)
}

/// Run `operation` under `captured`, if there is anything to apply.
///
/// [`Captured::NotCaptured`] and [`Captured::Absent`] both run `operation`
/// directly, leaving the calling thread's own context alone. That is what makes
/// applying a subset expressible, which the differing application windows of the
/// aspects require: a consumer may want impersonation around an open alone,
/// reverting immediately because later work uses the resulting handle and needs
/// no token.
///
/// # Errors
///
/// Returns [`ApplyError`] if the token could not be applied, in which case
/// `operation` did not run.
///
/// # Panics
///
/// Panics if the thread's entry context cannot be restored afterwards. This is
/// [`windows_impersonation_token_sys`]'s documented behaviour and is inherited
/// deliberately: a worker left under an unknown identity must not be returned to
/// shared infrastructure.
pub fn with_applied<F, T>(
    captured: &Captured<ImpersonationToken>,
    operation: F,
) -> Result<T, ApplyError>
where
    F: FnOnce() -> T,
{
    match captured.present() {
        Some(token) => token.with_impersonation(operation),
        None => Ok(operation()),
    }
}

#[cfg(test)]
mod tests;
