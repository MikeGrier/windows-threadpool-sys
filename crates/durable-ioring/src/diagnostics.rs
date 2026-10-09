// Copyright (c) 2026 Mike Grier
//! Where dioring reports what it observes about itself: a sink the consumer chooses (DI-D-51).
//!
//! The core reports typed events, never text, so a sink with a schema -- an ETW manifest -- maps
//! each one-to-one, and a sink that prints formats them as it likes. A sink states which kinds it
//! wants, and the core asks before it builds an event, so a sink that wants nothing costs a branch.

use std::fmt;
use std::sync::Arc;

#[cfg(test)]
pub(crate) mod tests;

/// What a [`Diagnostic`] is about: what a sink answers [`DiagnosticSink::wants`] for, before the
/// event is built. Open to further kinds -- the delay events of `DI-3.6` among them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// dioring found its own state inconsistent (DI-D-46).
    Inconsistency,
}

/// One event dioring reports. Open to further kinds, so match it with a wildcard arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum Diagnostic<'a> {
    /// An instance found its own state inconsistent -- a defect in dioring, or a layer beneath it
    /// breaking its contract, never the consumer's input -- and panics next (DI-D-46). Reported
    /// before the panic, and in its place while the thread is already unwinding.
    Inconsistency {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// What was inconsistent.
        what: &'static str,
        /// What was in hand when it was found: lineage, seal or epoch, operation, target or domain.
        detail: fmt::Arguments<'a>,
    },
}

impl Diagnostic<'_> {
    /// What the event is about.
    #[must_use]
    pub fn kind(&self) -> DiagnosticKind {
        match self {
            Self::Inconsistency { .. } => DiagnosticKind::Inconsistency,
        }
    }
}

impl fmt::Display for Diagnostic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inconsistency {
                instance,
                what,
                detail,
            } => write!(
                f,
                "instance {instance} found its own state inconsistent: {what} ({detail})"
            ),
        }
    }
}

/// Where an instance's diagnostics go (DI-D-51). Called from whatever thread observed the event,
/// with dioring's lock held, so a sink returns promptly and never calls back into the instance.
pub trait DiagnosticSink: Send + Sync {
    /// Whether the sink wants events of `kind`. Asked before each event is built, so answering
    /// `false` costs the core a branch and nothing more.
    fn wants(&self, kind: DiagnosticKind) -> bool;

    /// Report `event`, which is of a kind [`wants`](Self::wants) answered `true` for.
    fn report(&self, event: &Diagnostic<'_>);
}

/// A sink that wants nothing: the core's default.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDiagnostics;

impl DiagnosticSink for NoDiagnostics {
    fn wants(&self, _: DiagnosticKind) -> bool {
        false
    }

    fn report(&self, _: &Diagnostic<'_>) {}
}

/// A sink that writes every event as a line on standard error, prefixed `dioring:`.
#[derive(Clone, Copy, Debug, Default)]
pub struct StderrDiagnostics;

impl DiagnosticSink for StderrDiagnostics {
    fn wants(&self, _: DiagnosticKind) -> bool {
        true
    }

    fn report(&self, event: &Diagnostic<'_>) {
        eprintln!("dioring: {event}");
    }
}

/// A sink that hands each event to every sink it holds that wants its kind, in order.
#[derive(Clone, Default)]
pub struct FanOutDiagnostics {
    sinks: Vec<Arc<dyn DiagnosticSink>>,
}

impl FanOutDiagnostics {
    /// A fan-out to `sinks`, in order.
    #[must_use]
    pub fn new(sinks: Vec<Arc<dyn DiagnosticSink>>) -> Self {
        Self { sinks }
    }
}

impl fmt::Debug for FanOutDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FanOutDiagnostics")
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

impl DiagnosticSink for FanOutDiagnostics {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        self.sinks.iter().any(|sink| sink.wants(kind))
    }

    fn report(&self, event: &Diagnostic<'_>) {
        let kind = event.kind();
        for sink in self.sinks.iter().filter(|sink| sink.wants(kind)) {
            sink.report(event);
        }
    }
}
