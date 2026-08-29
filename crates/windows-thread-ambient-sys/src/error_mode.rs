// Copyright (c) Mike Grier.

//! The thread error mode aspect.
//!
//! The thread error mode decides whether a hard device error -- the classic
//! absent-removable-drive case -- raises a modal dialog or fails the call. A
//! thread-pool worker's mode is `0`, meaning the critical-error handler is
//! enabled, so a blocking call remoted onto shared infrastructure can put a
//! dialog on a thread the whole process depends on.
//!
//! # This aspect is both capturable and declarable, deliberately
//!
//! Unlike the others it appears in both halves of the crate's decomposition. It
//! is readable, so a caller may capture the submitting thread's value and
//! transplant it; and it is the aspect consumers most often want to *override*
//! with a policy of their own. Offering only one of those would bake one
//! consumer's answer into a platform layer: a consumer running on shared threads
//! will force the dialog-suppressing bits, while a consumer owning a private
//! thread, where a modal dialog is its own problem and nobody else's, is
//! entitled to the opposite choice.
//!
//! # Why the alignment bit is not representable
//!
//! [`ThreadErrorMode`] can hold only the three bits `SetThreadErrorMode`
//! accepts. `SEM_NOALIGNMENTFAULTEXCEPT` is excluded because it is *rejected*
//! per-thread -- and, measured, an invalid bit fails the **whole** call rather
//! than being dropped from it. A type that could represent it would let a caller
//! combine it with valid bits and silently lose the entire change, so this is a
//! case for a type that cannot express the invalid state rather than a runtime
//! check nobody expected to fail. See
//! [`windows-platform-probes`](../../windows-platform-probes/DESIGN-NOTES.md),
//! which pins the measurement as a test.
//!
//! # Example
//!
//! ```
//! use windows_thread_ambient_sys::ThreadErrorMode;
//!
//! let entry = ThreadErrorMode::capture()?;
//!
//! let mode = ThreadErrorMode::FAIL_CRITICAL_ERRORS
//!     .union(ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX);
//! let guard = mode.apply()?;
//! assert!(ThreadErrorMode::capture()?.contains(ThreadErrorMode::FAIL_CRITICAL_ERRORS));
//!
//! // Release explicitly. Dropping the guard also restores, but discards any
//! // failure to do so, because a destructor has no caller to report to.
//! guard.release()?;
//! assert_eq!(ThreadErrorMode::capture()?, entry);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;
use std::io;

use windows_sys::Win32::System::Diagnostics::Debug::{
    GetThreadErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX,
    SetThreadErrorMode, THREAD_ERROR_MODE,
};

/// Every bit this crate will place in a [`ThreadErrorMode`].
const SUPPORTED: THREAD_ERROR_MODE =
    SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX;

/// A thread error mode, restricted to the bits Windows accepts per thread.
///
/// Construct one from the associated constants and [`union`](Self::union), or
/// from a raw value with [`from_bits`](Self::from_bits).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ThreadErrorMode(THREAD_ERROR_MODE);

impl ThreadErrorMode {
    /// No bits set: hard errors raise the system's dialogs.
    pub const NONE: Self = Self(0);

    /// Fail calls that hit a critical error instead of raising a dialog.
    pub const FAIL_CRITICAL_ERRORS: Self = Self(SEM_FAILCRITICALERRORS);

    /// Suppress the general-protection-fault error box.
    pub const NO_GP_FAULT_ERROR_BOX: Self = Self(SEM_NOGPFAULTERRORBOX);

    /// Suppress the file-open error box.
    pub const NO_OPEN_FILE_ERROR_BOX: Self = Self(SEM_NOOPENFILEERRORBOX);

    /// The raw value, as Win32 represents it.
    #[must_use]
    pub const fn bits(self) -> THREAD_ERROR_MODE {
        self.0
    }

    /// Build a mode from a raw value.
    ///
    /// # Errors
    ///
    /// Returns [`UnsupportedBits`] if `bits` contains anything
    /// `SetThreadErrorMode` does not accept. This is a rejection rather than a
    /// mask: silently dropping a bit would report installing a value that was
    /// not installed, which is the failure this type exists to prevent.
    ///
    /// # Example
    ///
    /// ```
    /// use windows_thread_ambient_sys::ThreadErrorMode;
    ///
    /// assert_eq!(
    ///     ThreadErrorMode::from_bits(0x0001),
    ///     Ok(ThreadErrorMode::FAIL_CRITICAL_ERRORS)
    /// );
    ///
    /// // 0x0004 is SEM_NOALIGNMENTFAULTEXCEPT, which cannot be set per thread.
    /// // It is refused even beside a valid bit, because Windows would install
    /// // neither: an invalid bit fails the whole call.
    /// let refused = ThreadErrorMode::from_bits(0x0001 | 0x0004)
    ///     .expect_err("the alignment bit is not settable per thread");
    /// assert_eq!(refused.bits(), 0x0004);
    /// ```
    pub const fn from_bits(bits: THREAD_ERROR_MODE) -> Result<Self, UnsupportedBits> {
        let unsupported = bits & !SUPPORTED;
        if unsupported == 0 {
            Ok(Self(bits))
        } else {
            Err(UnsupportedBits { bits: unsupported })
        }
    }

    /// Both modes' bits.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit of `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no bits are set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Read the calling thread's current mode.
    ///
    /// # Errors
    ///
    /// Returns [`UnsupportedBits`] if Windows reports a bit this type cannot
    /// hold. Measured, that does not happen: the thread error mode is
    /// independent storage rather than a view of the process error mode, so a
    /// process-scope bit such as `SEM_NOALIGNMENTFAULTEXCEPT` does not show
    /// through here. The result is still surfaced rather than assumed away,
    /// because a type unable to represent a state the platform can produce would
    /// be a bug, and this is where that would first be observable.
    pub fn capture() -> Result<Self, UnsupportedBits> {
        // SAFETY: the call takes no arguments and has no preconditions.
        Self::from_bits(unsafe { GetThreadErrorMode() })
    }

    /// Install this mode on the calling thread until the guard is released.
    ///
    /// # Errors
    ///
    /// Returns [`ApplyError`] if Windows refuses the value, in which case
    /// nothing was installed and the thread is untouched.
    pub fn apply(self) -> Result<ErrorModeGuard, ApplyError> {
        let mut previous: THREAD_ERROR_MODE = 0;
        // SAFETY: `previous` is a valid writable destination, and `self.0`
        // cannot contain a bit the call rejects.
        let ok = unsafe { SetThreadErrorMode(self.0, &mut previous) };
        if ok == 0 {
            return Err(ApplyError {
                requested: self,
                source: io::Error::last_os_error(),
            });
        }
        Ok(ErrorModeGuard {
            previous,
            released: false,
        })
    }
}

impl fmt::Display for ThreadErrorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:04X}", self.0)
    }
}

/// A raw value contained bits the per-thread error mode does not accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UnsupportedBits {
    bits: THREAD_ERROR_MODE,
}

impl UnsupportedBits {
    /// Just the offending bits.
    #[must_use]
    pub const fn bits(self) -> THREAD_ERROR_MODE {
        self.bits
    }
}

impl fmt::Display for UnsupportedBits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "0x{:04X} cannot be set on a thread; SetThreadErrorMode rejects it \
             and would install none of the accompanying bits either",
            self.bits
        )
    }
}

impl std::error::Error for UnsupportedBits {}

/// Windows refused to install a thread error mode.
#[derive(Debug)]
pub struct ApplyError {
    requested: ThreadErrorMode,
    source: io::Error,
}

impl ApplyError {
    /// The mode that could not be installed.
    #[must_use]
    pub const fn requested(&self) -> ThreadErrorMode {
        self.requested
    }

    /// The underlying Win32 code, if there was one.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.raw_os_error()
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not install thread error mode {}: {}",
            self.requested, self.source
        )
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Windows refused to restore the thread's entry error mode.
#[derive(Debug)]
pub struct RestoreError {
    unrestored: THREAD_ERROR_MODE,
    source: io::Error,
}

impl RestoreError {
    /// The value the thread should have been returned to.
    #[must_use]
    pub const fn unrestored_bits(&self) -> THREAD_ERROR_MODE {
        self.unrestored
    }

    /// The underlying Win32 code, if there was one.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.raw_os_error()
    }
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not restore thread error mode 0x{:04X}; the thread is left \
             contaminated: {}",
            self.unrestored, self.source
        )
    }
}

impl std::error::Error for RestoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Holds an installed error mode until it is released.
///
/// # Release explicitly on the ordinary path
///
/// [`release`](Self::release) reports whether the thread was actually restored.
/// Dropping the guard instead restores on a best-effort basis and **discards**
/// any failure, because a destructor has no caller to report to. That is the
/// right behaviour while unwinding, where no report could be delivered anyway,
/// and the wrong behaviour on the ordinary path -- so the ordinary path calls
/// `release`.
///
/// Restoration failure is not fatal here. Contrast impersonation, whose restore
/// failure is fail-fast because returning a shared worker under an unknown
/// identity is a process-wide security failure; leaving a thread with the wrong
/// error mode is a real contamination but not that, and imposing the strictest
/// aspect's semantics on every aspect is precisely what this crate's composite
/// exists to avoid.
#[must_use = "dropping the guard restores the error mode but discards any failure to do so"]
#[derive(Debug)]
pub struct ErrorModeGuard {
    previous: THREAD_ERROR_MODE,
    released: bool,
}

impl ErrorModeGuard {
    /// The mode this thread had before the guard was installed.
    ///
    /// # Errors
    ///
    /// Returns [`UnsupportedBits`] in the same unreachable-by-measurement case
    /// as [`ThreadErrorMode::capture`]. The guard itself keeps the raw value, so
    /// restoration round-trips exactly whatever Windows reported, whether or not
    /// this crate's type can name it.
    pub const fn previous(&self) -> Result<ThreadErrorMode, UnsupportedBits> {
        ThreadErrorMode::from_bits(self.previous)
    }

    /// Restore the thread's entry mode, reporting whether it worked.
    ///
    /// # Errors
    ///
    /// Returns [`RestoreError`] if Windows refused, leaving the thread
    /// contaminated with whatever was installed.
    pub fn release(mut self) -> Result<(), RestoreError> {
        self.released = true;
        Self::restore(self.previous)
    }

    fn restore(previous: THREAD_ERROR_MODE) -> Result<(), RestoreError> {
        let mut ignored: THREAD_ERROR_MODE = 0;
        // SAFETY: `ignored` is a valid writable destination, and `previous` is a
        // value Windows itself reported for this thread.
        let ok = unsafe { SetThreadErrorMode(previous, &mut ignored) };
        if ok == 0 {
            return Err(RestoreError {
                unrestored: previous,
                source: io::Error::last_os_error(),
            });
        }
        Ok(())
    }
}

impl Drop for ErrorModeGuard {
    fn drop(&mut self) {
        if !self.released {
            // Best effort by design: see the type's documentation.
            let _ = Self::restore(self.previous);
        }
    }
}

#[cfg(test)]
mod tests;
