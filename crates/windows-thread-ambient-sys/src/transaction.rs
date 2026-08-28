// Copyright (c) Mike Grier.

//! The TxF transaction aspect.
//!
//! A thread may carry a *current transaction*, and `CreateFileW` and its
//! relatives silently join it. Remoting such a call to a worker that carries no
//! transaction would therefore perform it outside the caller's transaction --
//! quietly, and with no error to notice.
//!
//! # The documented entry points do not exist as exports
//!
//! `ktmw32.h` documents `GetCurrentTransaction` and `SetCurrentTransaction`, and
//! MSDN names `Ktmw32.dll` as their library. **Neither is exported from it** --
//! verified against the export table of the shipping DLL, which offers
//! `CreateTransaction`, `CommitTransaction`, `RollbackTransaction` and their
//! neighbours but nothing named `CurrentTransaction`. The header declares them
//! as `FORCEINLINE` wrappers, so a C caller links nothing; what actually carries
//! the operation is `RtlGetCurrentTransaction` / `RtlSetCurrentTransaction` in
//! **`ntdll.dll`**, and that is what this module binds.
//!
//! Two consequences worth stating plainly rather than discovering later. The
//! aspect depends on an `Rtl`-prefixed `ntdll` export rather than a documented
//! Win32 one -- unavoidable, since no documented export exists, but it is a
//! weaker footing than the rest of this crate and the reason binding is lazy and
//! failure is a typed `Unsupported` rather than a link error. And
//! `RtlSetCurrentTransaction` returns `BOOLEAN`, a **single byte**, not the
//! four-byte `BOOL` its documented wrapper returns; reading it as `BOOL` would
//! test three bytes of whatever happened to be in the register.
//!
//! Binding is lazy so that a consumer which never captures a transaction pays
//! nothing and, on a system where the symbols are absent, gets a typed failure
//! instead of a process that will not start. The module handle is deliberately
//! never freed: it is a process-lifetime binding resolved at most once, and
//! unloading it while another thread is inside a call would be a use-after-free
//! for no benefit.
//!
//! # The hazard this aspect cannot remove
//!
//! A transaction handle is a reference to a shared kernel object, so capturing
//! one does **not** give the worker a private transaction: the caller may commit
//! or roll it back while the worker is still inside it. Owning a duplicate fixes
//! only the *lifetime* problem -- the request cannot be left holding a closed
//! handle -- and not the *state* problem. Sequencing that is the consumer's
//! responsibility and cannot be enforced here.
//!
//! TxF is also deprecated by Microsoft. That is a reason to keep this aspect
//! optional and out of any minimal capture set, not a reason to omit it: a
//! caller using transacted NTFS today still needs its work remoted correctly.
//!
//! # Example
//!
//! ```
//! use windows_thread_ambient_sys::Captured;
//! use windows_thread_ambient_sys::transaction;
//!
//! // An ordinary thread carries no transaction. That is an *answer*, not a
//! // failure, so it is `Absent` rather than an error.
//! let captured = transaction::capture()?;
//! assert!(matches!(captured, Captured::Absent));
//!
//! // Applying `Absent` installs "no transaction" rather than leaving the
//! // running thread's own alone -- the caller asked, and the answer was none,
//! // so a worker that happened to carry one must not enlist this work in it.
//! let value = transaction::with_applied(&captured, || 42)?;
//! assert_eq!(value, 42);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::ffi::c_void;
use std::fmt;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, FALSE, HANDLE, HMODULE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::captured::Captured;

type GetCurrentTransactionFn = unsafe extern "system" fn() -> HANDLE;

/// Returns `BOOLEAN`, which is one byte. See the module documentation.
type SetCurrentTransactionFn = unsafe extern "system" fn(HANDLE) -> u8;

/// The lazily resolved thread-transaction entry points.
struct Ktm {
    get_current: GetCurrentTransactionFn,
    set_current: SetCurrentTransactionFn,
}

static KTM: OnceLock<Option<Ktm>> = OnceLock::new();

/// Resolve one symbol from a system DLL, loading it on first use.
///
/// Returns `None` if the library or the symbol is absent, which the aspect
/// reports as [`TransactionFailure::Unsupported`] rather than silently treating
/// as "no transaction" -- those are different facts.
///
/// Shared with this module's tests, which need `ktmw32.dll`'s transaction
/// *creation* entry points to exercise the non-empty path.
pub(crate) fn system_proc(dll: &str, name: &[u8]) -> Option<unsafe extern "system" fn() -> isize> {
    debug_assert_eq!(
        name.last(),
        Some(&0),
        "a symbol name must be NUL-terminated for GetProcAddress"
    );
    let wide: Vec<u16> = dll.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call. The handle is
    // intentionally never freed; see the module documentation.
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return None;
    }
    // SAFETY: `module` came from `LoadLibraryW` and is still loaded, and `name`
    // is NUL-terminated.
    unsafe { GetProcAddress(module as HMODULE, name.as_ptr()) }
}

fn ktm() -> Option<&'static Ktm> {
    KTM.get_or_init(|| {
        // The documented ktmw32 names are header inlines and are not exported;
        // these ntdll entry points are what they call.
        let get = system_proc("ntdll.dll", b"RtlGetCurrentTransaction\0")?;
        let set = system_proc("ntdll.dll", b"RtlSetCurrentTransaction\0")?;
        // SAFETY: both symbols are transmuted to the signatures ntdll declares
        // for them, including `RtlSetCurrentTransaction`'s single-byte BOOLEAN.
        unsafe {
            Some(Ktm {
                get_current: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    GetCurrentTransactionFn,
                >(get),
                set_current: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    SetCurrentTransactionFn,
                >(set),
            })
        }
    })
    .as_ref()
}

/// Whether this system offers the thread-transaction entry points at all.
#[must_use]
pub fn is_supported() -> bool {
    ktm().is_some()
}

/// Read the calling thread's current transaction, if any.
fn current_raw() -> Option<HANDLE> {
    let ktm = ktm()?;
    // SAFETY: the call takes no arguments and has no preconditions.
    let raw = unsafe { (ktm.get_current)() };
    Some(raw)
}

/// Is `raw` the "no transaction" sentinel?
fn is_none_sentinel(raw: HANDLE) -> bool {
    raw.is_null() || raw == INVALID_HANDLE_VALUE
}

/// An owned duplicate of a thread's current transaction.
#[derive(Debug)]
pub struct TransactionContext(OwnedHandle);

impl TransactionContext {
    /// The duplicated handle, for a consumer that must reach the raw object.
    #[must_use]
    pub fn as_raw(&self) -> HANDLE {
        self.0.as_raw_handle().cast::<c_void>()
    }
}

/// Why a transaction could not be captured or applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransactionFailure {
    /// `ktmw32.dll` or its thread-transaction entry points are unavailable.
    ///
    /// Distinct from "the thread had no transaction": one says the platform
    /// cannot answer, the other is an answer.
    Unsupported,
    /// The transaction handle could not be duplicated.
    Duplicate,
    /// Windows refused to install or restore a thread transaction.
    Install,
}

/// A transaction aspect operation failed.
#[derive(Debug)]
pub struct TransactionError {
    failure: TransactionFailure,
    source: Option<io::Error>,
}

impl TransactionError {
    /// Which stage failed.
    #[must_use]
    pub const fn failure(&self) -> TransactionFailure {
        self.failure
    }

    /// The underlying Win32 code, if there was one.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.as_ref().and_then(io::Error::raw_os_error)
    }
}

impl fmt::Display for TransactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.failure {
            TransactionFailure::Unsupported => {
                "ktmw32.dll does not offer the thread-transaction entry points"
            }
            TransactionFailure::Duplicate => "the transaction handle could not be duplicated",
            TransactionFailure::Install => "the thread transaction could not be set",
        };
        match &self.source {
            Some(source) => write!(f, "{what}: {source}"),
            None => f.write_str(what),
        }
    }
}

impl std::error::Error for TransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

fn error(failure: TransactionFailure) -> TransactionError {
    TransactionError {
        failure,
        source: Some(io::Error::last_os_error()),
    }
}

/// Capture the calling thread's current transaction.
///
/// Returns [`Captured::Absent`] when the thread carries no transaction, which is
/// the ordinary case, and [`Captured::Present`] holding an **owned duplicate**
/// otherwise, so the value does not depend on the caller keeping its own handle
/// open.
///
/// # Errors
///
/// Returns [`TransactionFailure::Unsupported`] if the entry points are
/// unavailable, and [`TransactionFailure::Duplicate`] if the handle could not be
/// duplicated. Neither is reported as "no transaction": a platform that cannot
/// answer is not the same as an answer.
pub fn capture() -> Result<Captured<TransactionContext>, TransactionError> {
    let raw = current_raw().ok_or(TransactionError {
        failure: TransactionFailure::Unsupported,
        source: None,
    })?;
    if is_none_sentinel(raw) {
        return Ok(Captured::Absent);
    }
    let mut duplicate: HANDLE = std::ptr::null_mut();
    // SAFETY: `raw` is the live handle Windows just reported for this thread,
    // `duplicate` is a valid writable destination, and both process handles are
    // pseudo-handles needing no cleanup.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut duplicate,
            0,
            FALSE,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(error(TransactionFailure::Duplicate));
    }
    // SAFETY: `DuplicateHandle` succeeded, so this handle is owned solely here.
    let owned = unsafe { OwnedHandle::from_raw_handle(duplicate.cast()) };
    Ok(Captured::Present(TransactionContext(owned)))
}

/// Install `raw` as the calling thread's transaction.
fn set_current(raw: HANDLE) -> Result<(), TransactionError> {
    let ktm = ktm().ok_or(TransactionError {
        failure: TransactionFailure::Unsupported,
        source: None,
    })?;
    // SAFETY: `raw` is either null (clearing the transaction) or a live handle.
    let ok = unsafe { (ktm.set_current)(raw) };
    if ok == 0 {
        return Err(error(TransactionFailure::Install));
    }
    Ok(())
}

/// Run `operation` under `captured`.
///
/// # `Absent` clears rather than leaving alone
///
/// This aspect is the first where [`Captured::Absent`] is reachable, so the
/// distinction between the two empty states has teeth here.
/// [`Captured::NotCaptured`] leaves the running thread's own transaction alone,
/// because the caller never asked. [`Captured::Absent`] *installs* "no
/// transaction", because the caller did ask and the answer was none -- and a
/// worker that happened to carry a transaction would otherwise silently enlist
/// the caller's work in it.
///
/// # Errors
///
/// Returns a [`TransactionError`] if the transaction could not be installed, in
/// which case `operation` did not run, or if the thread's entry transaction
/// could not be restored afterwards, in which case it did.
///
/// # Panics
///
/// Does not panic. Restore failure is reported rather than fatal, unlike
/// impersonation, whose fail-fast restore exists because an unknown *identity*
/// on a shared worker is a process-wide security failure. A stale transaction is
/// a real contamination but not that one.
pub fn with_applied<F, T>(
    captured: &Captured<TransactionContext>,
    operation: F,
) -> Result<T, TransactionError>
where
    F: FnOnce() -> T,
{
    let desired = match captured {
        Captured::NotCaptured => return Ok(operation()),
        Captured::Absent => std::ptr::null_mut(),
        Captured::Present(context) => context.as_raw(),
    };

    let previous = current_raw().ok_or(TransactionError {
        failure: TransactionFailure::Unsupported,
        source: None,
    })?;
    set_current(desired)?;

    let outcome = operation();

    // Restore whatever the thread had, including "none".
    let restore = set_current(if is_none_sentinel(previous) {
        std::ptr::null_mut()
    } else {
        previous
    });
    restore.map(|()| outcome)
}

#[cfg(test)]
mod tests;
