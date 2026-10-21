// Copyright (c) 2026 Mike Grier
//! Where dioring reports what it observes about itself: a sink the consumer chooses (DI-D-51).
//!
//! The core reports typed events, never text, so a sink with a schema -- an ETW manifest -- maps
//! each one-to-one, and a sink that prints formats them as it likes. A sink states which kinds it
//! wants, and the core asks before it builds an event, so a sink that wants nothing costs a branch.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::types::{FlushDomain, IoTargetKey};

#[cfg(test)]
pub(crate) mod tests;

/// What a [`Diagnostic`] is about: what a sink answers [`DiagnosticSink::wants`] for, before the
/// event is built and before any clock is read for it. Open to further kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// dioring found its own state inconsistent (DI-D-46).
    Inconsistency,
    /// A wait for one of an instance's locks that was not immediate (DI-D-26).
    LockWait,
    /// A seal's flushes, and its request to the consumer's provider, were issued (DI-D-26).
    FlushIssued,
    /// One part of a seal's flush answered (DI-D-26).
    FlushAnswered,
    /// A seal left waiting for the writes at or below it (DI-D-26).
    SealWaited,
}

/// Which part of a seal's flush an event is about.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum FlushPart<'a> {
    /// The built-in default's flush of one target.
    File(IoTargetKey),
    /// The consumer's provider's part for one flush domain.
    Domain(&'a FlushDomain),
}

impl fmt::Display for FlushPart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(key) => write!(f, "file {}", key.0),
            Self::Domain(domain) => {
                write!(f, "domain {:?}", String::from_utf8_lossy(domain.bytes()))
            }
        }
    }
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
    /// An acquisition of one of an instance's locks had to wait for it (DI-D-26). Reported after
    /// the lock was released, so the report does not lengthen the hold it measures. An
    /// acquisition that was immediate reports nothing.
    LockWait {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// Which lock: a name per lock.
        lock: &'static str,
        /// How long the wait was.
        waited: Duration,
    },
    /// A seal's flushes, and its request to the consumer's provider, were issued: the work that
    /// is a flush's delay when it is slow (DI-D-26).
    FlushIssued {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// The lineage, by its place among the instance's lineages: the default is 0.
        lineage: u64,
        /// The epoch the seal is through, as its identity renders itself.
        through: fmt::Arguments<'a>,
        /// How many targets the built-in default is asked to flush.
        files: usize,
        /// How many domains the consumer's provider is asked about.
        domains: usize,
    },
    /// One part of a seal's flush answered, which names the slow domain or target (DI-D-26).
    FlushAnswered {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// The lineage, by its place among the instance's lineages: the default is 0.
        lineage: u64,
        /// The epoch the seal is through, as its identity renders itself.
        through: fmt::Arguments<'a>,
        /// Which part answered.
        part: FlushPart<'a>,
        /// How long it took, from the part's being issued to its answer reaching the core.
        elapsed: Duration,
        /// Whether it answered with success.
        succeeded: bool,
    },
    /// A seal that had to wait for the writes at or below it -- those in flight and those held
    /// for a gate, which block it alike -- is no longer waiting, and its flushes are issued
    /// (DI-D-26's secondary event).
    SealWaited {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// The lineage, by its place among the instance's lineages: the default is 0.
        lineage: u64,
        /// The epoch the seal is through, as its identity renders itself.
        through: fmt::Arguments<'a>,
        /// How long from the seal's being asked for to its flushes being issued.
        waited: Duration,
    },
}

impl Diagnostic<'_> {
    /// What the event is about.
    #[must_use]
    pub fn kind(&self) -> DiagnosticKind {
        match self {
            Self::Inconsistency { .. } => DiagnosticKind::Inconsistency,
            Self::LockWait { .. } => DiagnosticKind::LockWait,
            Self::FlushIssued { .. } => DiagnosticKind::FlushIssued,
            Self::FlushAnswered { .. } => DiagnosticKind::FlushAnswered,
            Self::SealWaited { .. } => DiagnosticKind::SealWaited,
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
            Self::LockWait {
                instance,
                lock,
                waited,
            } => write!(
                f,
                "instance {instance} waited {waited:?} for its {lock} lock"
            ),
            Self::FlushIssued {
                instance,
                lineage,
                through,
                files,
                domains,
            } => write!(
                f,
                "instance {instance} lineage {lineage} seal through {through}: issued flushes of \
                 {files} files and {domains} domains"
            ),
            Self::FlushAnswered {
                instance,
                lineage,
                through,
                part,
                elapsed,
                succeeded,
            } => write!(
                f,
                "instance {instance} lineage {lineage} seal through {through}: flush of {part} \
                 {} after {elapsed:?}",
                if *succeeded { "succeeded" } else { "failed" }
            ),
            Self::SealWaited {
                instance,
                lineage,
                through,
                waited,
            } => write!(
                f,
                "instance {instance} lineage {lineage} seal through {through}: waited {waited:?} \
                 for its writes"
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

/// What a sink that writes an event as a line puts before it: the sinks that ship each write `dioring:`, a
/// space, and the event's text.
pub const LINE_PREFIX: &str = "dioring: ";

/// A sink that writes every event as a line on standard error, prefixed [`LINE_PREFIX`].
#[derive(Clone, Copy, Debug, Default)]
pub struct StderrDiagnostics;

impl DiagnosticSink for StderrDiagnostics {
    fn wants(&self, _: DiagnosticKind) -> bool {
        true
    }

    fn report(&self, event: &Diagnostic<'_>) {
        eprintln!("{LINE_PREFIX}{event}");
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
