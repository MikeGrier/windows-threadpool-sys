// Copyright (c) Mike Grier.

//! The composite: a thread's ambient state, captured as one value.
//!
//! [`AmbientState`] holds every aspect together, so a caller carries one value
//! to a worker rather than remembering which pieces it collected. Its field list
//! is contract surface: it is exhaustively enumerated, and a silently added
//! field would be a silent semantic change.
//!
//! # Capture fails on the calling thread, never later
//!
//! Capture is synchronous and happens where the caller can still act on the
//! result. A context that cannot be captured is an **admission** failure, not a
//! deferred one -- a worker discovering it later has no way to report it to
//! anyone who can do anything about it, and by then the caller has usually moved
//! on. The error names the aspect that failed, because "capture failed" is not
//! actionable when three aspects could have caused it.
//!
//! # Declared aspects are not captured
//!
//! [`Declared`] values are supplied by the caller and read from nothing, so they
//! are attached with [`AmbientState::with_declared`] rather than collected. That
//! separation is why the capture set names only capturable aspects.
//!
//! # Example
//!
//! ```
//! use windows_thread_ambient_sys::declared::MemoryPriority;
//! use windows_thread_ambient_sys::{AmbientState, CaptureSet, Declared};
//!
//! // Collected from this thread, right now, where a failure is still ours.
//! let state = AmbientState::capture(CaptureSet::DEFAULT)?
//!     // Stated rather than read: nothing was collected for this.
//!     .with_declared(Declared::none().with_memory_priority(MemoryPriority::Low));
//!
//! // What was asked for is recoverable afterwards, which is what keeps an
//! // omission distinguishable from an aspect that was captured and empty.
//! assert_eq!(state.captured_set(), CaptureSet::DEFAULT);
//! assert!(state.impersonation().was_captured());
//! assert!(!state.transaction().was_captured());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use windows_impersonation_token_sys::{
    CaptureError as ImpersonationCaptureError, ImpersonationToken,
};

use crate::capture_set::{CapturableAspect, CaptureSet};
use crate::captured::Captured;
use crate::declared::Declared;
use crate::error_mode::{ThreadErrorMode, UnsupportedBits};
use crate::transaction::{TransactionContext, TransactionError};
use crate::{impersonation, transaction};

/// Which aspect failed to capture, and why.
#[derive(Debug)]
#[non_exhaustive]
pub enum CaptureFailure {
    /// The impersonation context could not be captured.
    Impersonation(ImpersonationCaptureError),
    /// The thread error mode reported a value this crate cannot represent.
    ErrorMode(UnsupportedBits),
    /// The current transaction could not be captured.
    Transaction(TransactionError),
}

/// A composite capture failed.
///
/// The failing aspect is **derived** from the failure rather than stored beside
/// it, so the two cannot disagree.
#[derive(Debug)]
pub struct CaptureError {
    failure: CaptureFailure,
}

impl CaptureError {
    /// Which aspect failed.
    #[must_use]
    pub const fn aspect(&self) -> CapturableAspect {
        match self.failure {
            CaptureFailure::Impersonation(_) => CapturableAspect::Impersonation,
            CaptureFailure::ErrorMode(_) => CapturableAspect::ErrorMode,
            CaptureFailure::Transaction(_) => CapturableAspect::Transaction,
        }
    }

    /// The underlying failure.
    #[must_use]
    pub const fn failure(&self) -> &CaptureFailure {
        &self.failure
    }

    /// The underlying Win32 code, if the failing aspect reported one.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        match &self.failure {
            CaptureFailure::Impersonation(error) => error.raw_os_error(),
            CaptureFailure::ErrorMode(_) => None,
            CaptureFailure::Transaction(error) => error.raw_os_error(),
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "capturing the {} aspect failed: ", self.aspect())?;
        match &self.failure {
            CaptureFailure::Impersonation(error) => write!(f, "{error}"),
            CaptureFailure::ErrorMode(error) => write!(f, "{error}"),
            CaptureFailure::Transaction(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.failure {
            CaptureFailure::Impersonation(error) => Some(error),
            CaptureFailure::ErrorMode(error) => Some(error),
            CaptureFailure::Transaction(error) => Some(error),
        }
    }
}

/// A thread's ambient state, captured and declared, ready to travel.
///
/// The field list is exhaustive on purpose; see the module documentation.
#[derive(Debug)]
#[must_use = "an ambient state that is never applied captured a context for nothing"]
pub struct AmbientState {
    impersonation: Captured<ImpersonationToken>,
    error_mode: Captured<ThreadErrorMode>,
    transaction: Captured<TransactionContext>,
    declared: Declared,
}

impl AmbientState {
    /// Capture the aspects `set` names from the calling thread.
    ///
    /// Aspects outside `set` are [`Captured::NotCaptured`], which leaves the
    /// target thread's own value alone when the state is later applied -- a
    /// different thing from an aspect that was captured and found empty.
    ///
    /// Declared aspects are not touched here; attach them with
    /// [`with_declared`](Self::with_declared).
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError`], naming the aspect that failed. Any aspect
    /// captured before the failure is released rather than leaked, so a failed
    /// capture holds nothing.
    pub fn capture(set: CaptureSet) -> Result<Self, CaptureError> {
        // Order follows `CapturableAspect::EVERY` so the sequence is the one the
        // set reports, rather than an incidental one.
        let impersonation = if set.contains(CaptureSet::IMPERSONATION) {
            impersonation::capture().map_err(|error| CaptureError {
                failure: CaptureFailure::Impersonation(error),
            })?
        } else {
            Captured::NotCaptured
        };

        let error_mode = if set.contains(CaptureSet::ERROR_MODE) {
            Captured::Present(ThreadErrorMode::capture().map_err(|error| CaptureError {
                failure: CaptureFailure::ErrorMode(error),
            })?)
        } else {
            Captured::NotCaptured
        };

        let transaction = if set.contains(CaptureSet::TRANSACTION) {
            transaction::capture().map_err(|error| CaptureError {
                failure: CaptureFailure::Transaction(error),
            })?
        } else {
            Captured::NotCaptured
        };

        Ok(Self {
            impersonation,
            error_mode,
            transaction,
            declared: Declared::none(),
        })
    }

    /// Attach declared aspects, replacing any already attached.
    pub fn with_declared(mut self, declared: Declared) -> Self {
        self.declared = declared;
        self
    }

    /// What was actually collected.
    ///
    /// **Derived** from the aspects themselves rather than recorded separately,
    /// so it cannot disagree with what the state holds.
    #[must_use]
    pub fn captured_set(&self) -> CaptureSet {
        let mut set = CaptureSet::NONE;
        if self.impersonation.was_captured() {
            set = set.union(CaptureSet::IMPERSONATION);
        }
        if self.error_mode.was_captured() {
            set = set.union(CaptureSet::ERROR_MODE);
        }
        if self.transaction.was_captured() {
            set = set.union(CaptureSet::TRANSACTION);
        }
        set
    }

    /// The captured impersonation context.
    #[must_use]
    pub const fn impersonation(&self) -> &Captured<ImpersonationToken> {
        &self.impersonation
    }

    /// The captured thread error mode.
    #[must_use]
    pub const fn error_mode(&self) -> &Captured<ThreadErrorMode> {
        &self.error_mode
    }

    /// The captured transaction.
    #[must_use]
    pub const fn transaction(&self) -> &Captured<TransactionContext> {
        &self.transaction
    }

    /// The declared aspects.
    #[must_use]
    pub const fn declared(&self) -> &Declared {
        &self.declared
    }
}

#[cfg(test)]
mod tests;
