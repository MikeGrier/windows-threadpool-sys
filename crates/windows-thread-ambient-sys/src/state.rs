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
//! # One capture can serve many workers at once
//!
//! [`AmbientState`] is both `Send` and `Sync`, so a single capture may be shared
//! through an `Arc` and applied concurrently on any number of workers. That is
//! the shape a traversal or scan engine actually has: capture once at
//! submission, then run it on every worker for the length of the job. Each
//! application installs and restores on its own thread and observes nothing of
//! the others.
//!
//! Sharing is also the cheap option. Capture duplicates a kernel token object,
//! so re-capturing per unit of work re-pays for a snapshot the caller already
//! holds.
//!
//! # Granularity is the caller's choice, and it costs something
//!
//! Applying once around a batch of operations and applying once per operation
//! are both expressible, and the crate deliberately does not choose. Each
//! application is a `SetThreadToken` plus a call for every other aspect in play,
//! so a worker that opens a thousand files pays that a thousand times if it
//! applies per open.
//!
//! Prefer the widest window the aspects allow -- but note that the *narrowest*
//! window is sometimes the correct one for a reason unrelated to cost:
//! [crates/windows-file-enumeration-sys](../../windows-file-enumeration-sys/DESIGN-NOTES.md)
//! deliberately impersonates only around its directory open, because every later
//! query uses the resulting handle and needs no token at all. Holding a token
//! longer than the work requires is a security decision, not just a performance
//! one.
//!
//! # The blast radius of fail-fast restoration
//!
//! A failure to restore impersonation panics. **That is this crate's decision,
//! not one inherited from a dependency**: a shared worker returned to a pool
//! under an unknown identity is a process-wide security failure, and no error
//! return could make a caller notice in time.
//! [`windows_impersonation_token_sys`] is used because its behaviour already
//! satisfies that requirement; if it stopped doing so, the dependency would be
//! wrong and this guarantee would stay.
//!
//! The consequence is worth stating plainly for anyone running many impersonated
//! workers. A panic inside a thread-pool callback **aborts the process** -- the
//! pool has no caller to unwind to -- so a restore failure on one worker of
//! sixty-four is not one failed operation, it is the whole process. This is the
//! intended trade, and a consumer that cannot accept it should not be applying
//! impersonation on threads it does not own.
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
//!
//! // Applying installs every aspect in a fixed order and releases in exact
//! // reverse. An uncaptured aspect is skipped, leaving the running thread's own
//! // value alone.
//! let applied = state.with_applied(|| "work")?;
//! assert_eq!(*applied.value(), "work");
//! assert!(applied.restore().is_clean());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use windows_impersonation_token_sys::{
    ApplyError as ImpersonationApplyError, CaptureError as ImpersonationCaptureError,
    ImpersonationToken,
};

use crate::capture_set::{CapturableAspect, CaptureSet};
use crate::captured::Captured;
use crate::declared::{Declared, DeclaredError};
use crate::error_mode::{
    ApplyError as ErrorModeApplyError, ErrorModeGuard, RestoreError as ErrorModeRestoreError,
    ThreadErrorMode, UnsupportedBits,
};
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

    /// Run `operation` with this state installed on the calling thread.
    ///
    /// # Order
    ///
    /// Guards are applied outermost-first and released in **exact reverse**, so
    /// the thread passes back through each intermediate state:
    ///
    /// 1. thread error mode -- outermost, so hard-error suppression is already
    ///    in force while everything else is being applied;
    /// 2. declared aspects (background mode, memory priority, redirection);
    /// 3. TxF transaction;
    /// 4. impersonation -- innermost, because its window is the narrowest and
    ///    its restoration is the one that must not be delayed.
    ///
    /// Applying a subset stays expressible: an aspect that is
    /// [`Captured::NotCaptured`] or unspecified is skipped entirely, leaving the
    /// running thread's own value alone.
    ///
    /// # Overriding rather than transplanting the error mode
    ///
    /// This applies the error mode it *captured*. A consumer that wants to
    /// impose its own -- forcing the dialog-suppressing bits on a shared worker,
    /// say -- should leave [`CaptureSet::ERROR_MODE`] out of its capture set and
    /// wrap this call in its own [`ThreadErrorMode::apply`] guard, which then
    /// sits outermost, exactly where the order above puts it. Capturing *and*
    /// overriding would install the captured value inside the override.
    ///
    /// # Errors
    ///
    /// Returns [`ApplyError`] if an aspect could not be installed, in which case
    /// `operation` did not run and every already-installed aspect is released
    /// first.
    ///
    /// A failure to **restore** is different, and does not fail the call: the
    /// operation ran and its value is kept, with the failures reported through
    /// [`Applied::restore`]. Discarding a successful operation's value because a
    /// priority could not be put back would lose more than it protects.
    ///
    /// # Panics
    ///
    /// **Panics if the impersonation context cannot be restored, and that is
    /// this crate's guarantee rather than a detail of its dependencies.**
    /// Returning a shared worker to a pool under an unknown identity is a
    /// process-wide security failure: every later task on that thread would run
    /// as whoever the failed restore left behind, and no caller could detect it
    /// from a returned error. Failing fast is the only response that cannot be
    /// ignored, which is a different order of hazard from the other aspects
    /// here, and they are reported rather than fatal.
    ///
    /// [`windows_impersonation_token_sys`] is used because its behaviour
    /// already satisfies that guarantee. If it ever stopped doing so, the
    /// dependency would be wrong and this contract would not change -- an
    /// earlier version of this note described the semantics as *inherited* from
    /// that crate and "not chosen here", which left a security property of this
    /// public API resting on someone else's implementation detail.
    pub fn with_applied<F, T>(&self, operation: F) -> Result<Applied<T>, ApplyError>
    where
        F: FnOnce() -> T,
    {
        // 1. Error mode, outermost.
        let error_mode_guard = match self.error_mode.present() {
            Some(mode) => Some(mode.apply().map_err(|error| ApplyError {
                failure: ApplyFailure::ErrorMode(error),
            })?),
            None => None,
        };

        // 2. Declared aspects.
        let declared_guard = match self.declared.install() {
            Ok(guard) => guard,
            Err(error) => {
                release_error_mode(error_mode_guard);
                return Err(ApplyError {
                    failure: ApplyFailure::Declared(error),
                });
            }
        };

        // 3. Transaction.
        let transaction_guard = match transaction::install(&self.transaction) {
            Ok(guard) => guard,
            Err(error) => {
                drop(declared_guard);
                release_error_mode(error_mode_guard);
                return Err(ApplyError {
                    failure: ApplyFailure::Transaction(error),
                });
            }
        };

        // 4. Impersonation, innermost, and closure-scoped by its own crate.
        let outcome = impersonation::with_applied(&self.impersonation, operation);
        let value = match outcome {
            Ok(value) => value,
            Err(error) => {
                drop(transaction_guard);
                drop(declared_guard);
                release_error_mode(error_mode_guard);
                return Err(ApplyError {
                    failure: ApplyFailure::Impersonation(error),
                });
            }
        };

        // Release in exact reverse. Every release is attempted even after one
        // fails, because stopping early leaves more of the thread contaminated.
        //
        // These are separate statements rather than a struct literal on purpose:
        // the order below *is* the release order, and burying it in field
        // initialisers would make a later reader's harmless-looking field
        // reordering silently reorder the releases.
        let transaction = transaction_guard.release().err();
        let declared = declared_guard.release().err();
        let error_mode = match error_mode_guard {
            Some(guard) => guard.release().err(),
            None => None,
        };

        Ok(Applied {
            value,
            restore: RestoreReport {
                error_mode,
                declared,
                transaction,
            },
        })
    }
}

fn release_error_mode(guard: Option<ErrorModeGuard>) {
    if let Some(guard) = guard {
        // Best effort: an install failed, so this path already has an error to
        // report and a second one would displace it.
        let _ = guard.release();
    }
}

/// What an operation produced, and whether the thread was put back.
#[derive(Debug)]
#[must_use = "ignoring the restore report discards evidence that the thread is contaminated"]
pub struct Applied<T> {
    value: T,
    restore: RestoreReport,
}

impl<T> Applied<T> {
    /// The operation's value.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Take the value, deliberately ignoring the restore report.
    pub fn into_value(self) -> T {
        self.value
    }

    /// Which aspects failed to restore, if any.
    pub const fn restore(&self) -> &RestoreReport {
        &self.restore
    }

    /// Take the value only if the thread was restored cleanly.
    ///
    /// # Errors
    ///
    /// Returns the report when any aspect failed to restore. The value is
    /// dropped in that case, so a caller that needs both should use
    /// [`value`](Self::value) and [`restore`](Self::restore) instead.
    pub fn into_clean_value(self) -> Result<T, RestoreReport> {
        if self.restore.is_clean() {
            Ok(self.value)
        } else {
            Err(self.restore)
        }
    }
}

/// Which aspects could not be restored after an operation.
///
/// Exhaustively enumerated rather than a list, so a reader can see every aspect
/// that can appear without running anything. Impersonation is absent by
/// construction: its restore failure is fatal, so it never reaches a report.
#[derive(Debug, Default)]
pub struct RestoreReport {
    error_mode: Option<ErrorModeRestoreError>,
    declared: Option<DeclaredError>,
    transaction: Option<TransactionError>,
}

impl RestoreReport {
    /// Whether every aspect was restored.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.error_mode.is_none() && self.declared.is_none() && self.transaction.is_none()
    }

    /// The thread error mode's restore failure, if any.
    #[must_use]
    pub const fn error_mode(&self) -> Option<&ErrorModeRestoreError> {
        self.error_mode.as_ref()
    }

    /// The declared aspects' restore failure, if any.
    #[must_use]
    pub const fn declared(&self) -> Option<&DeclaredError> {
        self.declared.as_ref()
    }

    /// The transaction's restore failure, if any.
    #[must_use]
    pub const fn transaction(&self) -> Option<&TransactionError> {
        self.transaction.as_ref()
    }
}

impl fmt::Display for RestoreReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_clean() {
            return f.write_str("the thread was restored cleanly");
        }
        f.write_str("the thread is contaminated:")?;
        if let Some(error) = &self.error_mode {
            write!(f, " error mode: {error};")?;
        }
        if let Some(error) = &self.declared {
            write!(f, " declared: {error};")?;
        }
        if let Some(error) = &self.transaction {
            write!(f, " transaction: {error};")?;
        }
        Ok(())
    }
}

impl std::error::Error for RestoreReport {}

/// Which aspect could not be installed, and why.
#[derive(Debug)]
#[non_exhaustive]
pub enum ApplyFailure {
    /// The thread error mode could not be installed.
    ErrorMode(ErrorModeApplyError),
    /// A declared aspect could not be installed.
    Declared(DeclaredError),
    /// The transaction could not be installed.
    Transaction(TransactionError),
    /// The impersonation context could not be applied.
    Impersonation(ImpersonationApplyError),
}

/// Applying a composite state failed, so the operation did not run.
#[derive(Debug)]
pub struct ApplyError {
    failure: ApplyFailure,
}

impl ApplyError {
    /// The underlying failure, whose variant names the aspect.
    #[must_use]
    pub const fn failure(&self) -> &ApplyFailure {
        &self.failure
    }

    /// The underlying Win32 code, if the failing aspect reported one.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        match &self.failure {
            ApplyFailure::ErrorMode(error) => error.raw_os_error(),
            ApplyFailure::Declared(error) => error.raw_os_error(),
            ApplyFailure::Transaction(error) => error.raw_os_error(),
            ApplyFailure::Impersonation(error) => error.raw_os_error(),
        }
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("applying the ambient state failed: ")?;
        match &self.failure {
            ApplyFailure::ErrorMode(error) => write!(f, "{error}"),
            ApplyFailure::Declared(error) => write!(f, "{error}"),
            ApplyFailure::Transaction(error) => write!(f, "{error}"),
            ApplyFailure::Impersonation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.failure {
            ApplyFailure::ErrorMode(error) => Some(error),
            ApplyFailure::Declared(error) => Some(error),
            ApplyFailure::Transaction(error) => Some(error),
            ApplyFailure::Impersonation(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests;
