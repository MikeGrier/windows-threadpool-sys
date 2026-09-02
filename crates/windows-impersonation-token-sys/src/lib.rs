// Copyright (c) 2026 Mike Grier
//! Memory-safe capture, transport, and scoped application of Windows
//! impersonation tokens.
//!
//! [`ImpersonationToken`] captures the calling thread's effective security
//! context into an owned impersonation token that can be carried across
//! threads without exposing its raw handle or mutation rights.
//!
//! # Capture contract
//!
//! Capture is synchronous. If the calling thread is impersonating, the
//! captured token preserves its identification, impersonation, or delegation
//! level. If the thread has no token, capture snapshots the process identity as
//! a `SecurityImpersonation` token. Windows does not permit anonymous
//! impersonation contexts to be opened, so they are rejected as
//! [`CaptureFailure::AnonymousContext`].
//!
//! # Application and restoration
//!
//! [`ImpersonationToken::with_impersonation`] applies the captured token only
//! for the dynamic extent of a closure. Before applying it, the method opens a
//! handle to the exact thread-token object present on entry, or records that
//! the thread had no token. That exact state is restored on ordinary return and
//! during unwinding; restoration never substitutes a duplicate token and never
//! uses `RevertToSelf`.
//!
//! A failure to save the entry state or apply the captured token is returned as
//! [`ApplyError`] before the closure runs. A failure to restore the entry state
//! panics because continuing to use a shared worker thread under an unknown
//! identity would be unsafe. If the closure is already unwinding, the resulting
//! double panic aborts the process.
//!
//! # Security invariants
//!
//! - The captured handle is owned, non-inheritable, and grants only
//!   `TOKEN_IMPERSONATE`.
//! - Clones share the same immutable token object; no safe API exposes its
//!   handle or permits token mutation or rights expansion.
//! - The private restoration guard cannot move to or be shared with another
//!   thread.
//! - The public API is closure-only, so safe code cannot forget the restoration
//!   guard.
//!
//! # Example
//!
//! Capture once, move the owned token to a worker, and scope access-checked work
//! to that context:
//!
//! ```no_run
//! use std::thread;
//! use windows_impersonation_token_sys::ImpersonationToken;
//!
//! let token = ImpersonationToken::capture()?;
//! let worker = thread::spawn(move || {
//!     token.with_impersonation(|| {
//!         // Perform access-checked Windows work here.
//!     })
//! });
//!
//! worker.join().expect("worker panicked")?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![cfg(windows)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod restore;

use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{ERROR_CANT_OPEN_ANONYMOUS, ERROR_NO_TOKEN, FALSE, TRUE};
use windows_sys::Win32::Security::{
    DuplicateTokenEx, GetTokenInformation, SECURITY_IMPERSONATION_LEVEL, SecurityAnonymous,
    SecurityImpersonation, TOKEN_ACCESS_MASK, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY,
    TokenImpersonation, TokenImpersonationLevel,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken, SetThreadToken,
};

const THREAD_TOKEN_CAPTURE_ACCESS: TOKEN_ACCESS_MASK = TOKEN_DUPLICATE | TOKEN_QUERY;

// A mutation run reports the `|` above as replaceable by `^`, and it is an
// equivalent mutant rather than a gap: these are single-bit access rights, so
// the operands share no set bit and `|` and `^` agree. No test can distinguish
// them.
//
// The assertion is what makes that argument checkable rather than merely
// stated. If a future access mask were folded in that overlapped -- a composite
// right such as `TOKEN_READ`, which includes `TOKEN_QUERY` -- the equivalence
// would quietly stop holding and `^` would start *clearing* a bit the capture
// needs. That failure is invisible at the call site and would surface as a
// permission error much later, so it is caught here at compile time instead.
const _: () = assert!(
    TOKEN_DUPLICATE & TOKEN_QUERY == 0,
    "the capture mask combines single-bit rights; overlapping bits would make \
     the combinator load-bearing rather than a spelling choice"
);
const PROCESS_TOKEN_CAPTURE_ACCESS: TOKEN_ACCESS_MASK = TOKEN_DUPLICATE;
const CAPTURED_TOKEN_ACCESS: TOKEN_ACCESS_MASK = TOKEN_IMPERSONATE;

/// The stage at which an impersonation context could not be captured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CaptureFailure {
    /// The current thread is impersonating at `SecurityAnonymous`, whose token
    /// Windows does not permit callers to open or transport.
    AnonymousContext,
    /// Windows could not open the current thread token.
    OpenThreadToken,
    /// The thread had no token and Windows could not open the process token.
    OpenProcessToken,
    /// Windows could not report the thread token's impersonation level.
    QueryImpersonationLevel,
    /// Windows could not duplicate the effective token into the captured form.
    DuplicateToken,
}

/// A synchronous failure while capturing the calling thread's effective
/// impersonation context.
#[derive(Debug)]
pub struct CaptureError {
    failure: CaptureFailure,
    source: io::Error,
}

impl CaptureError {
    fn new(failure: CaptureFailure, source: io::Error) -> Self {
        Self { failure, source }
    }

    fn anonymous() -> Self {
        Self::new(
            CaptureFailure::AnonymousContext,
            io::Error::from_raw_os_error(
                i32::try_from(ERROR_CANT_OPEN_ANONYMOUS)
                    .expect("ERROR_CANT_OPEN_ANONYMOUS fits in i32"),
            ),
        )
    }

    /// The capture stage that failed.
    #[must_use]
    pub fn failure(&self) -> CaptureFailure {
        self.failure
    }

    /// The underlying Win32 error code.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.raw_os_error()
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stage = match self.failure {
            CaptureFailure::AnonymousContext => "anonymous impersonation context",
            CaptureFailure::OpenThreadToken => "OpenThreadToken",
            CaptureFailure::OpenProcessToken => "OpenProcessToken",
            CaptureFailure::QueryImpersonationLevel => {
                "GetTokenInformation(TokenImpersonationLevel)"
            }
            CaptureFailure::DuplicateToken => "DuplicateTokenEx",
        };

        write!(f, "{stage}: {}", self.source)
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// The stage at which a captured token could not be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ApplyFailure {
    /// Windows could not open the current thread token for exact restoration.
    SavePreviousToken,
    /// Windows could not assign the captured token to the current thread.
    ApplyToken,
}

/// A synchronous failure while applying a captured impersonation context.
#[derive(Debug)]
pub struct ApplyError {
    failure: ApplyFailure,
    source: io::Error,
}

impl ApplyError {
    fn new(failure: ApplyFailure, source: io::Error) -> Self {
        Self { failure, source }
    }

    /// The application stage that failed.
    #[must_use]
    pub fn failure(&self) -> ApplyFailure {
        self.failure
    }

    /// The underlying Win32 error code.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.raw_os_error()
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stage = match self.failure {
            ApplyFailure::SavePreviousToken => "OpenThreadToken for exact restoration",
            ApplyFailure::ApplyToken => "SetThreadToken",
        };

        write!(f, "{stage}: {}", self.source)
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// An owned, immutable snapshot of a Windows impersonation context.
///
/// Cloning this value shares ownership of the same captured token. The native
/// handle is real rather than pseudo or borrowed, is not inheritable, and has
/// only `TOKEN_IMPERSONATE` access. No safe API exposes that handle or permits
/// callers to mutate the captured token. The value is safe to send to and share
/// between threads; each call to [`Self::with_impersonation`] affects only the
/// calling thread for the duration of its closure.
#[must_use = "dropping the token discards the captured impersonation context"]
pub struct ImpersonationToken {
    handle: Arc<OwnedHandle>,
}

impl Clone for ImpersonationToken {
    fn clone(&self) -> Self {
        Self {
            handle: Arc::clone(&self.handle),
        }
    }
}

impl ImpersonationToken {
    /// Captures the calling thread's effective Windows security context.
    ///
    /// If the thread is impersonating, capture preserves its identification,
    /// impersonation, or delegation level. If the thread has no token, capture
    /// snapshots the process token as a `SecurityImpersonation` token. The
    /// captured handle is non-inheritable and grants only `TOKEN_IMPERSONATE`.
    ///
    /// # Errors
    ///
    /// Returns a [`CaptureError`] synchronously when the effective token cannot
    /// be opened, inspected, or duplicated. Anonymous impersonation is reported
    /// as [`CaptureFailure::AnonymousContext`].
    pub fn capture() -> Result<Self, CaptureError> {
        let source = SourceToken::open()?;
        let mut captured = ptr::null_mut();

        // SAFETY: source.handle is a live token handle with TOKEN_DUPLICATE;
        // captured points to writable storage. Null security attributes make
        // the new TOKEN_IMPERSONATE-only handle non-inheritable.
        let duplicated = unsafe {
            DuplicateTokenEx(
                source.handle.as_raw_handle(),
                CAPTURED_TOKEN_ACCESS,
                ptr::null(),
                source.level,
                TokenImpersonation,
                &raw mut captured,
            )
        };
        if duplicated == FALSE {
            return Err(CaptureError::new(
                CaptureFailure::DuplicateToken,
                io::Error::last_os_error(),
            ));
        }

        // SAFETY: successful DuplicateTokenEx returns a new, owned token handle
        // that must be released with CloseHandle, which OwnedHandle does.
        let handle = unsafe { OwnedHandle::from_raw_handle(captured) };
        Ok(Self {
            handle: Arc::new(handle),
        })
    }

    /// Runs `operation` with this token applied to the current thread.
    ///
    /// The exact prior thread-token state is restored before this method
    /// returns, whether `operation` returns an ordinary value or a `Result`.
    /// Stack unwinding also restores the prior state. The closure's return value
    /// is not interpreted, so a fallible closure produces
    /// `Result<Result<T, E>, ApplyError>`.
    ///
    /// This method changes only the calling thread's impersonation state. The
    /// token may be reused sequentially or concurrently on other threads.
    ///
    /// # Errors
    ///
    /// Returns an [`ApplyError`] before calling `operation` when the exact prior
    /// context cannot be saved or the captured token cannot be applied.
    ///
    /// # Panics
    ///
    /// Panics if restoring the exact prior thread-token state fails. Returning
    /// a shared worker thread under an unknown identity would permit unrelated
    /// later work to run with the wrong security context. If restoration fails
    /// while `operation` is already unwinding, Rust's double-panic behavior
    /// aborts the process.
    pub fn with_impersonation<F, T>(&self, operation: F) -> Result<T, ApplyError>
    where
        F: FnOnce() -> T,
    {
        run_in_scope(ApplicationGuard::apply(self), operation)
    }
}

impl fmt::Debug for ImpersonationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImpersonationToken").finish_non_exhaustive()
    }
}

fn run_in_scope<G, F, T>(guard: Result<G, ApplyError>, operation: F) -> Result<T, ApplyError>
where
    F: FnOnce() -> T,
{
    let guard = guard?;
    let result = operation();
    drop(guard);
    Ok(result)
}

struct ApplicationGuard {
    previous: Option<OwnedHandle>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl ApplicationGuard {
    fn apply(token: &ImpersonationToken) -> Result<Self, ApplyError> {
        let previous = open_previous_token()?;

        // SAFETY: a null thread pointer selects the current thread. The
        // captured handle remains live and has TOKEN_IMPERSONATE access.
        let applied = unsafe { SetThreadToken(ptr::null(), token.handle.as_raw_handle()) };
        check_application_result(applied, io::Error::last_os_error)?;

        Ok(Self {
            previous,
            _thread_bound: PhantomData,
        })
    }
}

impl Drop for ApplicationGuard {
    fn drop(&mut self) {
        let previous = self
            .previous
            .as_ref()
            .map_or(ptr::null_mut(), AsRawHandle::as_raw_handle);

        // SAFETY: a null thread pointer selects the current thread. previous is
        // either null (restore no-token process context) or a live token handle
        // opened with TOKEN_IMPERSONATE. The Rc marker prevents this guard from
        // moving to or being shared with another thread.
        let restored = unsafe { SetThreadToken(ptr::null(), previous) };
        if restored == FALSE {
            restore::panic_failure(io::Error::last_os_error());
        }
    }
}

fn check_application_result<F>(applied: i32, last_error: F) -> Result<(), ApplyError>
where
    F: FnOnce() -> io::Error,
{
    if applied == FALSE {
        Err(ApplyError::new(ApplyFailure::ApplyToken, last_error()))
    } else {
        Ok(())
    }
}

fn open_previous_token() -> Result<Option<OwnedHandle>, ApplyError> {
    let mut raw = ptr::null_mut();

    // SAFETY: GetCurrentThread is a valid pseudo-handle for this call and raw
    // points to writable handle storage. OpenAsSelf permits saving an
    // identification-level token using the process context for the access check.
    let opened =
        unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_IMPERSONATE, TRUE, &raw mut raw) };
    if opened != FALSE {
        // SAFETY: successful OpenThreadToken returns a new, owned handle closed
        // by OwnedHandle.
        return Ok(Some(unsafe { OwnedHandle::from_raw_handle(raw) }));
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error()
        == Some(i32::try_from(ERROR_NO_TOKEN).expect("ERROR_NO_TOKEN fits in i32"))
    {
        Ok(None)
    } else {
        Err(ApplyError::new(ApplyFailure::SavePreviousToken, error))
    }
}

struct SourceToken {
    handle: OwnedHandle,
    level: SECURITY_IMPERSONATION_LEVEL,
}

impl SourceToken {
    fn open() -> Result<Self, CaptureError> {
        let mut raw = ptr::null_mut();

        // SAFETY: GetCurrentThread is a valid pseudo-handle for this call and
        // raw points to writable handle storage. OpenAsSelf uses the process
        // context for the access check, which is required for identification
        // level impersonation.
        let opened = unsafe {
            OpenThreadToken(
                GetCurrentThread(),
                THREAD_TOKEN_CAPTURE_ACCESS,
                TRUE,
                &raw mut raw,
            )
        };
        if opened != FALSE {
            // SAFETY: successful OpenThreadToken returns a new, owned handle
            // closed by OwnedHandle.
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            let level = query_impersonation_level(&handle)?;
            return Ok(Self { handle, level });
        }

        let error = io::Error::last_os_error();
        match classify_thread_token_open_error(error) {
            ThreadTokenOpenError::NoToken => Self::open_process(),
            ThreadTokenOpenError::Capture(error) => Err(error),
        }
    }

    fn open_process() -> Result<Self, CaptureError> {
        let mut raw = ptr::null_mut();

        // SAFETY: GetCurrentProcess is a valid pseudo-handle for this call and
        // raw points to writable handle storage.
        let opened = unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                PROCESS_TOKEN_CAPTURE_ACCESS,
                &raw mut raw,
            )
        };
        if opened == FALSE {
            return Err(CaptureError::new(
                CaptureFailure::OpenProcessToken,
                io::Error::last_os_error(),
            ));
        }

        // SAFETY: successful OpenProcessToken returns a new, owned handle
        // closed by OwnedHandle.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        Ok(Self {
            handle,
            level: SecurityImpersonation,
        })
    }
}

fn query_impersonation_level(
    handle: &OwnedHandle,
) -> Result<SECURITY_IMPERSONATION_LEVEL, CaptureError> {
    let mut level = SecurityAnonymous;
    let mut returned = 0;
    let level_size =
        u32::try_from(size_of::<SECURITY_IMPERSONATION_LEVEL>()).expect("token level fits in u32");

    // SAFETY: handle has TOKEN_QUERY and remains live for the call. level is a
    // correctly sized writable SECURITY_IMPERSONATION_LEVEL buffer and returned
    // points to writable length storage.
    let queried = unsafe {
        GetTokenInformation(
            handle.as_raw_handle(),
            TokenImpersonationLevel,
            (&raw mut level).cast(),
            level_size,
            &raw mut returned,
        )
    };
    if queried == FALSE {
        return Err(CaptureError::new(
            CaptureFailure::QueryImpersonationLevel,
            io::Error::last_os_error(),
        ));
    }
    if level == SecurityAnonymous {
        return Err(CaptureError::anonymous());
    }

    Ok(level)
}

enum ThreadTokenOpenError {
    NoToken,
    Capture(CaptureError),
}

fn classify_thread_token_open_error(error: io::Error) -> ThreadTokenOpenError {
    match error.raw_os_error() {
        Some(code)
            if code
                == i32::try_from(ERROR_CANT_OPEN_ANONYMOUS)
                    .expect("ERROR_CANT_OPEN_ANONYMOUS fits in i32") =>
        {
            ThreadTokenOpenError::Capture(CaptureError::new(
                CaptureFailure::AnonymousContext,
                error,
            ))
        }
        Some(code)
            if code == i32::try_from(ERROR_NO_TOKEN).expect("ERROR_NO_TOKEN fits in i32") =>
        {
            ThreadTokenOpenError::NoToken
        }
        _ => {
            ThreadTokenOpenError::Capture(CaptureError::new(CaptureFailure::OpenThreadToken, error))
        }
    }
}

#[cfg(test)]
mod tests;
