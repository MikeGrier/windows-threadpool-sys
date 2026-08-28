// Copyright (c) Mike Grier.

//! The declared aspects: WOW64 filesystem redirection, memory priority, and I/O
//! priority.
//!
//! These are *declared*, not captured. A declared aspect has nothing to collect
//! from the calling thread, so it is not part of any capture set: the caller
//! states the value it wants installed, and leaving it unspecified means the
//! running thread's own value is untouched.
//!
//! # Why each one is declared, and the reasons differ
//!
//! Stating this per aspect rather than as one blanket claim matters, because the
//! reasons are not the same and only one of them is absolute.
//!
//! - **WOW64 filesystem redirection has no getter at all.**
//!   `Wow64DisableWow64FsRedirection` yields an `OldValue` only as a side effect
//!   of *disabling* redirection, and there is no way to observe the current
//!   state without changing it. An aspect that cannot be read cannot be
//!   transplanted; this is a mechanical impossibility, not a preference.
//! - **Memory priority is readable**, through
//!   `GetThreadInformation(ThreadMemoryPriority)`, so this one is a choice.
//!   Priority is a policy about how work should compete for resources, not
//!   something a caller implicitly consents to having remoted -- and silently
//!   transplanting it is not the safe default either way, since a caller in a
//!   background mode would have its remoted work quietly promoted or demoted
//!   without saying so.
//! - **I/O priority has no documented getter**, and does not move on its own: it
//!   changes only in lockstep with CPU and memory priority, through background
//!   mode. So there is no independent value to capture even in principle, and
//!   declaring background mode declares all three together, which the type says
//!   out loud rather than hiding.
//!
//! # Redirection only does anything in a 32-bit process
//!
//! `Wow64DisableWow64FsRedirection` is meaningful only for a 32-bit process on
//! 64-bit Windows. In a 64-bit process there is no redirector to disable and the
//! call fails. That failure is reported rather than swallowed, because a caller
//! that asked for redirection to be disabled and silently did not get it would
//! be reading a different filesystem than it believes.
//!
//! # Example
//!
//! ```
//! use windows_thread_ambient_sys::Declared;
//! use windows_thread_ambient_sys::declared::MemoryPriority;
//!
//! let entry = MemoryPriority::current()?;
//!
//! // Only what is named is installed; every other aspect is left alone.
//! let declared = Declared::none().with_memory_priority(MemoryPriority::Low);
//!
//! let during = declared.with_applied(MemoryPriority::current)?;
//! assert_eq!(during?, MemoryPriority::Low);
//!
//! // The thread is returned to whatever it had, not to an assumed default.
//! assert_eq!(MemoryPriority::current()?, entry);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;
use std::io;

use windows_sys::Win32::Storage::FileSystem::{
    Wow64DisableWow64FsRedirection, Wow64RevertWow64FsRedirection,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, GetThreadInformation, MEMORY_PRIORITY, MEMORY_PRIORITY_BELOW_NORMAL,
    MEMORY_PRIORITY_INFORMATION, MEMORY_PRIORITY_LOW, MEMORY_PRIORITY_MEDIUM,
    MEMORY_PRIORITY_NORMAL, MEMORY_PRIORITY_VERY_LOW, SetThreadInformation, SetThreadPriority,
    THREAD_MODE_BACKGROUND_BEGIN, THREAD_MODE_BACKGROUND_END, ThreadMemoryPriority,
};

/// How a thread's memory pages compete for physical memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MemoryPriority {
    /// The lowest priority; pages are trimmed first.
    VeryLow,
    /// Below `BelowNormal`.
    Low,
    /// Between `Low` and `BelowNormal`.
    Medium,
    /// Below the default.
    BelowNormal,
    /// The default for a thread.
    Normal,
}

impl MemoryPriority {
    /// The Win32 value.
    #[must_use]
    pub const fn as_raw(self) -> MEMORY_PRIORITY {
        match self {
            Self::VeryLow => MEMORY_PRIORITY_VERY_LOW,
            Self::Low => MEMORY_PRIORITY_LOW,
            Self::Medium => MEMORY_PRIORITY_MEDIUM,
            Self::BelowNormal => MEMORY_PRIORITY_BELOW_NORMAL,
            Self::Normal => MEMORY_PRIORITY_NORMAL,
        }
    }

    /// Interpret a Win32 value.
    #[must_use]
    pub const fn from_raw(raw: MEMORY_PRIORITY) -> Option<Self> {
        match raw {
            MEMORY_PRIORITY_VERY_LOW => Some(Self::VeryLow),
            MEMORY_PRIORITY_LOW => Some(Self::Low),
            MEMORY_PRIORITY_MEDIUM => Some(Self::Medium),
            MEMORY_PRIORITY_BELOW_NORMAL => Some(Self::BelowNormal),
            MEMORY_PRIORITY_NORMAL => Some(Self::Normal),
            _ => None,
        }
    }

    /// Read the calling thread's current memory priority.
    ///
    /// Provided because the value *is* readable, which is what makes this
    /// aspect's declared status a deliberate choice rather than a limitation. A
    /// consumer that decides transplanting is right for it can read the value
    /// here and declare it; the crate simply does not do so on the caller's
    /// behalf.
    ///
    /// # Errors
    ///
    /// Returns [`DeclaredError`] if the query fails or reports a value this
    /// enumeration does not name.
    pub fn current() -> Result<Self, DeclaredError> {
        let mut info = MEMORY_PRIORITY_INFORMATION {
            MemoryPriority: MEMORY_PRIORITY_NORMAL,
        };
        // SAFETY: the destination is a valid, correctly sized struct and the
        // pseudo-handle needs no cleanup.
        let ok = unsafe {
            GetThreadInformation(
                GetCurrentThread(),
                ThreadMemoryPriority,
                std::ptr::from_mut(&mut info).cast(),
                size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(DeclaredError::new(DeclaredAspect::MemoryPriority));
        }
        Self::from_raw(info.MemoryPriority)
            .ok_or_else(|| DeclaredError::without_os_error(DeclaredAspect::MemoryPriority))
    }

    fn install(self) -> Result<(), DeclaredError> {
        let info = MEMORY_PRIORITY_INFORMATION {
            MemoryPriority: self.as_raw(),
        };
        // SAFETY: the source is a valid, correctly sized struct and the
        // pseudo-handle needs no cleanup.
        let ok = unsafe {
            SetThreadInformation(
                GetCurrentThread(),
                ThreadMemoryPriority,
                std::ptr::from_ref(&info).cast(),
                size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(DeclaredError::new(DeclaredAspect::MemoryPriority));
        }
        Ok(())
    }
}

/// Whether the thread runs in background processing mode.
///
/// Background mode is not an I/O-priority knob on its own: entering it lowers
/// CPU, I/O **and** memory priority together, and leaving it restores all three.
/// The name says so rather than presenting it as an I/O setting that happens to
/// have side effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackgroundMode {
    /// Enter background processing mode for the operation.
    Begin,
    /// Leave background processing mode for the operation.
    End,
}

impl BackgroundMode {
    fn install(self) -> Result<(), DeclaredError> {
        let value = match self {
            Self::Begin => THREAD_MODE_BACKGROUND_BEGIN,
            Self::End => THREAD_MODE_BACKGROUND_END,
        };
        // SAFETY: the pseudo-handle needs no cleanup and `value` is a documented
        // background-mode selector.
        let ok = unsafe { SetThreadPriority(GetCurrentThread(), value) };
        if ok == 0 {
            return Err(DeclaredError::new(DeclaredAspect::BackgroundMode));
        }
        Ok(())
    }

    /// The call that undoes this one.
    const fn inverse(self) -> Self {
        match self {
            Self::Begin => Self::End,
            Self::End => Self::Begin,
        }
    }
}

/// What to do with WOW64 filesystem redirection.
///
/// Only [`Disabled`](Self::Disabled) is expressible, because Windows offers no
/// way to *enable* redirection that was never disabled, and no way to read the
/// current state. The enum exists rather than a bare `bool` so that a later
/// capability does not change the meaning of an existing value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Wow64Redirection {
    /// Disable redirection for the operation, restoring it afterwards.
    Disabled,
}

/// Which declared aspect an operation concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DeclaredAspect {
    /// WOW64 filesystem redirection.
    Wow64Redirection,
    /// Thread memory priority.
    MemoryPriority,
    /// Background processing mode.
    BackgroundMode,
}

/// A declared aspect could not be installed or restored.
#[derive(Debug)]
pub struct DeclaredError {
    aspect: DeclaredAspect,
    source: Option<io::Error>,
}

impl DeclaredError {
    fn new(aspect: DeclaredAspect) -> Self {
        Self {
            aspect,
            source: Some(io::Error::last_os_error()),
        }
    }

    const fn without_os_error(aspect: DeclaredAspect) -> Self {
        Self {
            aspect,
            source: None,
        }
    }

    /// Which aspect failed.
    #[must_use]
    pub const fn aspect(&self) -> DeclaredAspect {
        self.aspect
    }

    /// The underlying Win32 code, if there was one.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.as_ref().and_then(io::Error::raw_os_error)
    }
}

impl fmt::Display for DeclaredError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.aspect {
            DeclaredAspect::Wow64Redirection => {
                "WOW64 filesystem redirection could not be changed (it exists only \
                 for a 32-bit process on 64-bit Windows)"
            }
            DeclaredAspect::MemoryPriority => "the thread memory priority could not be read or set",
            DeclaredAspect::BackgroundMode => "background processing mode could not be changed",
        };
        match &self.source {
            Some(source) => write!(f, "{what}: {source}"),
            None => f.write_str(what),
        }
    }
}

impl std::error::Error for DeclaredError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// The declared aspects a caller wants installed.
///
/// Every field is optional, and `None` means **leave the running thread's own
/// value alone**. That is not the same as declaring a default: a declared
/// default would overwrite whatever the thread had.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Declared {
    /// WOW64 filesystem redirection.
    pub wow64_redirection: Option<Wow64Redirection>,
    /// Thread memory priority.
    pub memory_priority: Option<MemoryPriority>,
    /// Background processing mode, which moves CPU, I/O and memory priority
    /// together.
    pub background_mode: Option<BackgroundMode>,
}

impl Declared {
    /// Declare nothing, leaving every aspect of the running thread alone.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            wow64_redirection: None,
            memory_priority: None,
            background_mode: None,
        }
    }

    /// Whether anything at all would be installed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.wow64_redirection.is_none()
            && self.memory_priority.is_none()
            && self.background_mode.is_none()
    }

    /// Declare WOW64 filesystem redirection.
    #[must_use]
    pub const fn with_wow64_redirection(mut self, value: Wow64Redirection) -> Self {
        self.wow64_redirection = Some(value);
        self
    }

    /// Declare a memory priority.
    #[must_use]
    pub const fn with_memory_priority(mut self, value: MemoryPriority) -> Self {
        self.memory_priority = Some(value);
        self
    }

    /// Declare a background processing mode.
    #[must_use]
    pub const fn with_background_mode(mut self, value: BackgroundMode) -> Self {
        self.background_mode = Some(value);
        self
    }

    /// Run `operation` with these aspects installed.
    ///
    /// Guards are applied in a fixed order and released in exact reverse, so the
    /// thread passes back through each intermediate state rather than being
    /// snapped to an assumed one.
    ///
    /// # Errors
    ///
    /// Returns [`DeclaredError`] if an aspect could not be installed, in which
    /// case `operation` did not run and every already-installed aspect is
    /// released first; or if an aspect could not be restored afterwards, in
    /// which case it did run and the thread is left contaminated.
    pub fn with_applied<F, T>(&self, operation: F) -> Result<T, DeclaredError>
    where
        F: FnOnce() -> T,
    {
        let background = match self.background_mode {
            Some(mode) => {
                mode.install()?;
                Some(mode)
            }
            None => None,
        };

        let memory = match self.memory_priority {
            Some(priority) => {
                let previous = MemoryPriority::current();
                match previous.and_then(|previous| priority.install().map(|()| previous)) {
                    Ok(previous) => Some(previous),
                    Err(error) => {
                        release_background(background);
                        return Err(error);
                    }
                }
            }
            None => None,
        };

        let redirection = match self.wow64_redirection {
            Some(Wow64Redirection::Disabled) => {
                let mut old: *mut core::ffi::c_void = std::ptr::null_mut();
                // SAFETY: `old` is a valid writable destination, and is passed
                // back verbatim to the revert call below.
                let ok = unsafe { Wow64DisableWow64FsRedirection(&mut old) };
                if ok == 0 {
                    let error = DeclaredError::new(DeclaredAspect::Wow64Redirection);
                    release_memory(memory);
                    release_background(background);
                    return Err(error);
                }
                Some(old)
            }
            None => None,
        };

        let outcome = operation();

        // Exact reverse order.
        let mut failure = None;
        if let Some(old) = redirection {
            // SAFETY: `old` is the value the matching disable call produced.
            let ok = unsafe { Wow64RevertWow64FsRedirection(old) };
            if ok == 0 {
                failure = Some(DeclaredError::new(DeclaredAspect::Wow64Redirection));
            }
        }
        if let Some(previous) = memory
            && let Err(error) = previous.install()
        {
            failure = failure.or(Some(error));
        }
        if let Some(mode) = background
            && let Err(error) = mode.inverse().install()
        {
            failure = failure.or(Some(error));
        }

        match failure {
            Some(error) => Err(error),
            None => Ok(outcome),
        }
    }
}

fn release_background(background: Option<BackgroundMode>) {
    if let Some(mode) = background {
        let _ = mode.inverse().install();
    }
}

fn release_memory(memory: Option<MemoryPriority>) {
    if let Some(previous) = memory {
        let _ = previous.install();
    }
}

#[cfg(test)]
mod tests;
