// Copyright (c) Mike Grier.

//! What ambient state a thread-pool worker starts with.
//!
//! The context-capture design rests on two claims about a worker thread the
//! process did not create and does not own:
//!
//! - it does **not** inherit the submitter's impersonation token, so identity
//!   must be captured and applied explicitly;
//! - its thread error mode leaves the critical-error handler **enabled**, so a
//!   hard device error can put a modal dialog on process-shared infrastructure.
//!
//! Both are the reason `windows-thread-ambient-sys` exists at all. Measured
//! rather than reasoned, because "a fresh thread starts clean" is exactly the
//! kind of plausible claim that turns out to have an exception.
//!
//! Migrated from the throwaway `ctx-probe` spike (Probe E).

use std::ffi::c_void;
use std::sync::mpsc::{Sender, channel};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NO_TOKEN, GetLastError, HANDLE};
use windows_sys::Win32::Security::{
    DuplicateTokenEx, RevertToSelf, SecurityImpersonation, TOKEN_ALL_ACCESS, TokenImpersonation,
};
use windows_sys::Win32::System::Diagnostics::Debug::GetThreadErrorMode;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken, PTP_CALLBACK_INSTANCE,
    SetThreadToken, TP_CALLBACK_ENVIRON_V3, TrySubmitThreadpoolCallback,
};

/// What a worker observed about the thread it was handed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerContext {
    /// Whether `OpenThreadToken` found a token on the worker.
    pub has_thread_token: bool,
    /// `GetLastError` from that call when it found none.
    ///
    /// `ERROR_NO_TOKEN` is the answer that means "this thread is not
    /// impersonating", as opposed to a failure to ask.
    pub open_token_error: u32,
    /// The worker's thread error mode on entry.
    pub error_mode: u32,
}

impl WorkerContext {
    /// The worker started with no impersonation token at all.
    ///
    /// Distinguished from "could not tell": only `ERROR_NO_TOKEN` means the
    /// thread genuinely had none.
    #[must_use]
    pub fn is_unimpersonated(self) -> bool {
        !self.has_thread_token && self.open_token_error == ERROR_NO_TOKEN
    }

    /// The critical-error handler is enabled on this worker.
    ///
    /// `SEM_FAILCRITICALERRORS` *suppresses* the handler, so its absence is
    /// what leaves a modal dialog reachable. An error mode of zero is the
    /// worst case and the one measured.
    #[must_use]
    pub fn critical_error_handler_enabled(self) -> bool {
        self.error_mode & crate::error_mode::bits::FAIL_CRITICAL_ERRORS == 0
    }
}

/// Reads the calling thread's ambient state.
fn observe_here() -> WorkerContext {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentThread returns a pseudo-handle needing no cleanup, and
    // `token` is a writable destination.
    let opened =
        unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_ALL_ACCESS, 1, &raw mut token) };
    let open_token_error = if opened == 0 {
        // SAFETY: no preconditions.
        unsafe { GetLastError() }
    } else {
        0
    };
    if opened != 0 && !token.is_null() {
        // SAFETY: a successful OpenThreadToken yields a handle this call owns.
        unsafe { CloseHandle(token) };
    }

    // SAFETY: no preconditions; the mode is the return value.
    let error_mode = unsafe { GetThreadErrorMode() };

    WorkerContext {
        has_thread_token: opened != 0,
        open_token_error,
        error_mode,
    }
}

/// The callback the pool runs, which reports what it found and returns.
///
/// # Safety
///
/// `context` must be a `Sender<WorkerContext>` leaked by [`observe_on_worker`],
/// and this must run exactly once so the boxed sender is freed once.
unsafe extern "system" fn report(_instance: PTP_CALLBACK_INSTANCE, context: *mut c_void) {
    // SAFETY: the caller guarantees the pointer came from Box::into_raw on a
    // Sender and that this runs once, so reclaiming it here is sound.
    let sender = unsafe { Box::from_raw(context.cast::<Sender<WorkerContext>>()) };
    let _ = sender.send(observe_here());
}

/// Submits one callback to the **process** thread pool and reports what the
/// worker found.
///
/// The process pool is deliberate: it is the shared infrastructure the design
/// worries about, and a private pool would not answer the question a consumer
/// actually faces.
///
/// # Panics
///
/// Panics if the callback cannot be submitted or never reports, since a probe
/// that silently measured nothing would be worse than one that stopped.
#[must_use]
pub fn observe_on_worker() -> WorkerContext {
    let (sender, receiver) = channel();
    let boxed = Box::into_raw(Box::new(sender));

    // SAFETY: `report` matches the callback signature and `boxed` is the
    // leaked Sender it expects; a null environment selects the process pool.
    let submitted = unsafe {
        TrySubmitThreadpoolCallback(
            Some(report),
            boxed.cast::<c_void>(),
            std::ptr::null::<TP_CALLBACK_ENVIRON_V3>(),
        )
    };
    assert_ne!(submitted, 0, "submit a callback to the process thread pool");

    receiver
        .recv()
        .expect("the worker reported what it was handed")
}

/// Submits a callback **while the submitting thread is impersonating**, and
/// reports what the worker found.
///
/// This is the measurement that matters: a worker that inherited the
/// submitter's token would make explicit capture unnecessary, and one that does
/// not is why the ambient crate exists.
///
/// # Panics
///
/// Panics if the process token cannot be duplicated or applied, or if the
/// worker never reports.
#[must_use]
pub fn observe_on_worker_while_impersonating() -> WorkerContext {
    let mut process_token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; `process_token` is
    // writable.
    let opened = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ALL_ACCESS,
            &raw mut process_token,
        )
    };
    assert_ne!(opened, 0, "open the process token");

    let mut impersonation: HANDLE = std::ptr::null_mut();
    // SAFETY: `process_token` is live; null attributes make the duplicate
    // non-inheritable; `impersonation` is writable.
    let duplicated = unsafe {
        DuplicateTokenEx(
            process_token,
            TOKEN_ALL_ACCESS,
            std::ptr::null(),
            SecurityImpersonation,
            TokenImpersonation,
            &raw mut impersonation,
        )
    };
    // SAFETY: the process token handle is no longer needed either way.
    unsafe { CloseHandle(process_token) };
    assert_ne!(
        duplicated, 0,
        "duplicate the process token for impersonation"
    );

    // SAFETY: `impersonation` is a live impersonation token.
    let applied = unsafe { SetThreadToken(std::ptr::null(), impersonation) };
    assert_ne!(applied, 0, "impersonate on the submitting thread");

    // The submitter is now genuinely impersonating, which is the premise.
    assert!(
        observe_here().has_thread_token,
        "the submitting thread must actually hold a token, or the probe proves nothing"
    );

    let observed = observe_on_worker();

    // SAFETY: restores the thread to its own identity; the token handle is then
    // no longer needed.
    unsafe {
        RevertToSelf();
        CloseHandle(impersonation);
    }

    observed
}
