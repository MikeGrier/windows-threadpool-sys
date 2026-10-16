// Copyright (c) 2026 Mike Grier
//! Where the watcher reports what it observes about itself: a sink the caller chooses (TW-D-13).
//!
//! The shape is that of durable-ioring's `DiagnosticSink`: typed events and not text, so a sink with
//! a schema maps each one to its own record; a sink says which kinds it wants before an event is
//! built, so one that wants nothing costs a branch; and the default wants nothing. The shape is
//! mirrored and the types are this crate's own.

use std::fmt;
use std::sync::Arc;

#[cfg(test)]
mod tests;

/// What a [`Diagnostic`] is about: what a sink is asked about, through
/// [`DiagnosticSink::wants`], before the event is built. Open to further kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// A probe of a registration failed in a way that may not recur.
    ProbeFailed,
}

/// One event the watcher reports. Open to further kinds, so match it with a wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Diagnostic {
    /// A probe of a registration failed in a way that may not recur. Nothing is done about it, and
    /// the registration is probed again on the next beat. A failure that cannot recur is reported
    /// to the caller as an SQ entry instead.
    ProbeFailed {
        /// The registration, by [`Registration::id`](crate::Registration::id).
        registration: u64,
        /// The Win32 code the failure carried, if it carried one.
        win32_code: Option<u32>,
    },
}

impl Diagnostic {
    /// What the event is about.
    #[must_use]
    pub fn kind(&self) -> DiagnosticKind {
        match self {
            Self::ProbeFailed { .. } => DiagnosticKind::ProbeFailed,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProbeFailed {
                registration,
                win32_code: Some(code),
            } => write!(
                f,
                "probing registration {registration} failed with Win32 code {code}; it will be probed again"
            ),
            Self::ProbeFailed {
                registration,
                win32_code: None,
            } => write!(
                f,
                "probing registration {registration} failed with an error that carries no Win32 code; it will be probed again"
            ),
        }
    }
}

/// Where a watcher's diagnostics go. Called from whatever thread observed the event, possibly while
/// the watcher holds a lock of its own, so a sink returns promptly and never calls back into the
/// watcher.
pub trait DiagnosticSink: Send + Sync {
    /// Whether the sink wants events of `kind`. Asked before each event is built, so answering
    /// `false` costs the watcher a branch and nothing more.
    fn wants(&self, kind: DiagnosticKind) -> bool;

    /// Report `event`, which is of a kind [`wants`](Self::wants) answered `true` for.
    fn report(&self, event: &Diagnostic);
}

/// A sink that wants nothing: the default.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDiagnostics;

impl DiagnosticSink for NoDiagnostics {
    fn wants(&self, _: DiagnosticKind) -> bool {
        false
    }

    fn report(&self, _: &Diagnostic) {}
}

/// The watcher's sink, shared by everything that reports.
#[derive(Clone)]
pub(crate) struct SharedSink(Arc<dyn DiagnosticSink>);

impl SharedSink {
    pub(crate) fn new(sink: Arc<dyn DiagnosticSink>) -> Self {
        Self(sink)
    }

    /// Report the event `make` builds, if the sink wants `kind`. `make` is not called otherwise.
    pub(crate) fn emit(&self, kind: DiagnosticKind, make: impl FnOnce() -> Diagnostic) {
        if self.0.wants(kind) {
            self.0.report(&make());
        }
    }
}

impl fmt::Debug for SharedSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SharedSink")
    }
}
