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

#![cfg(windows)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
