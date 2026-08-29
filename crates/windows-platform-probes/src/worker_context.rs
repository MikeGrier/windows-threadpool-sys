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
    DuplicateTokenEx, RevertToSelf, SecurityImpersonation, TOKEN_ALL_ACCESS, TOKEN_QUERY,
    TokenImpersonation,
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

/// What the submitter and its worker each saw, at the same moment.
///
/// The asymmetry is the finding, so both sides are returned. Reporting only
/// the worker would leave "the submitter really was impersonating" to an
/// assertion buried inside the probe, which a test cannot observe -- and a
/// test that cannot observe its own control is asserting the conclusion twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityAsymmetry {
    /// What the submitting thread saw about itself while impersonating.
    pub submitter: WorkerContext,
    /// What the worker that ran its callback saw.
    pub worker: WorkerContext,
}

impl IdentityAsymmetry {
    /// The submitter held a token and the worker did not.
    ///
    /// Both halves matter: without the first, "the worker has no token"
    /// proves nothing, because nobody had one.
    #[must_use]
    pub fn disagree(self) -> bool {
        self.submitter.has_thread_token && self.worker.is_unimpersonated()
    }
}

/// Reads the calling thread's ambient state.
///
/// # Why the token is opened for `TOKEN_QUERY` only
///
/// This asks one question: is there a token at all? `TOKEN_QUERY` is the least
/// access that answers it. Asking for `TOKEN_ALL_ACCESS` -- which this did
/// until a review caught it -- makes the call fail with `ERROR_ACCESS_DENIED`
/// against a token that exists but does not grant everything, which this probe
/// would then report as `has_thread_token: false` with an error that is not
/// `ERROR_NO_TOKEN`.
///
/// That conflates "no token" with "a token I could not open that widely", and
/// the distinction is the entire content of [`is_unimpersonated`]. The failure
/// would surface as an asserted-tier test failing for a reason having nothing
/// to do with the platform behaviour being measured.
///
/// [`is_unimpersonated`]: WorkerContext::is_unimpersonated
fn observe_here() -> WorkerContext {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentThread returns a pseudo-handle needing no cleanup, and
    // `token` is a writable destination.
    let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &raw mut token) };
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
    if submitted == 0 {
        // The callback was never queued, so `report` -- the only other path
        // that reclaims this box -- will never run. Taking it back here is
        // what keeps a failed submission from leaking permanently in a
        // long-running process.
        // SAFETY: `boxed` came from Box::into_raw above and nothing else owns
        // it, precisely because the submission failed.
        drop(unsafe { Box::from_raw(boxed) });
        panic!("submit a callback to the process thread pool");
    }

    receiver
        .recv()
        .expect("the worker reported what it was handed")
}

/// Impersonates this thread for as long as it is held, then reverts.
///
/// # Why a guard rather than a revert after the measurement
///
/// The measurement between the two is not panic-free. `observe_on_worker`
/// panics if the pool refuses the callback or the worker never reports, and
/// the submitter-token assert panics when the premise fails -- which is
/// exactly when something has already gone wrong. A plain `RevertToSelf` after
/// the measurement is skipped on every one of those paths, leaving the thread
/// impersonating and the token leaked.
///
/// That matters more than an ordinary leak because of how these probes run:
/// under `--test-threads=1` libtest executes tests inline on the calling
/// thread, so a leaked identity is inherited by every later test, and
/// `a_thread_pool_worker_starts_with_no_impersonation_token` asserts the
/// opposite as its precondition. The original failure would be buried under a
/// cascade of unrelated ones.
///
/// The same defect was fixed in this workspace's `while_impersonating` test
/// helper; this is its production sibling, and it is fixed the same way.
pub(crate) struct Impersonation(HANDLE);

impl Impersonation {
    /// Duplicates the process token and applies it to the calling thread.
    ///
    /// # Panics
    ///
    /// Panics if the token cannot be opened, duplicated, or applied. The
    /// duplicate is owned before it is applied, so it is closed even when
    /// applying it fails.
    pub(crate) fn apply() -> Self {
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

        // Owned from here, so the assert below cannot leak the duplicate.
        let guard = Self(impersonation);

        // SAFETY: `impersonation` is a live impersonation token.
        let applied = unsafe { SetThreadToken(std::ptr::null(), guard.0) };
        assert_ne!(applied, 0, "impersonate on the submitting thread");

        guard
    }
}

impl Drop for Impersonation {
    fn drop(&mut self) {
        // SAFETY: restores this thread to its own identity, and closes the
        // duplicate this value owns exactly once. Reverting a thread that is
        // not impersonating -- the case where `SetThreadToken` failed -- is
        // harmless.
        unsafe {
            RevertToSelf();
            CloseHandle(self.0);
        }
    }
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
pub fn observe_on_worker_while_impersonating() -> IdentityAsymmetry {
    // Reverts and closes on drop, including while unwinding out of either
    // panic below. See `Impersonation`.
    let _impersonation = Impersonation::apply();

    // The submitter is now genuinely impersonating, which is the premise. It is
    // both asserted here -- so the probe fails fast rather than reporting a
    // meaningless asymmetry -- and returned, so a caller can assert it too.
    let submitter = observe_here();
    assert!(
        submitter.has_thread_token,
        "the submitting thread must actually hold a token, or the probe proves nothing"
    );

    let worker = observe_on_worker();

    IdentityAsymmetry { submitter, worker }
}
