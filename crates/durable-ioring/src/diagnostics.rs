// Copyright (c) 2026 Mike Grier
//! Where dioring reports what it observes about itself: a sink the consumer chooses (DI-D-51).
//!
//! The core reports typed events, never text, so a sink with a schema -- an ETW manifest -- maps
//! each one-to-one, and a sink that prints formats them as it likes. A sink states which kinds it
//! wants, and the core asks before it builds an event, so a sink that wants nothing costs a branch.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::correlation::CorrelationId;
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
    /// A seal, from its being asked for to its being answered: a pair (DI-D-80, DI-D-87).
    Seal,
    /// One part of a seal's flush, from its issue to its answer: a pair (DI-D-80, DI-D-87).
    Flush,
}

/// Which end of a pair an event is (DI-D-80).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edge {
    /// The pair's first event.
    Begin,
    /// The pair's last event: nothing of the pair follows it.
    End,
}

/// What makes an event one end of a pair: the pair's kind, its id, and which end. The one
/// definition of which events pair, which the pairing checker and a sink that marks the ends
/// both read (DI-D-87).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pairing {
    /// The pair's kind, which its begin and its end share.
    pub kind: DiagnosticKind,
    /// The pair's id, which its begin and its end share.
    pub id: CorrelationId,
    /// Which end this is.
    pub edge: Edge,
}

/// How a seal's pair ended (DI-D-87).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SealOutcome {
    /// The seal was answered durable.
    Durable,
    /// The seal was answered with a failure of its own flush.
    Failed,
    /// The seal was answered blocked by an earlier failure.
    Blocked,
    /// The seal was never answered: its lineage ended, or its instance was closed or dropped.
    Ended,
}

impl fmt::Display for SealOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Durable => "durable",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Ended => "ended unanswered",
        })
    }
}

/// How a flush's pair ended (DI-D-87).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FlushOutcome {
    /// The part answered success.
    Succeeded,
    /// The part answered a failure.
    Failed,
    /// The part was never answered: its completion was dropped, its lineage ended, or its
    /// instance was closed or dropped.
    Abandoned,
}

impl fmt::Display for FlushOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        })
    }
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
    /// A seal was asked for: the begin of its pair (DI-D-87).
    SealBegan {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// The lineage, by its place among the instance's lineages: the default is 0.
        lineage: u64,
        /// The epoch the seal is through, as its identity renders itself.
        through: fmt::Arguments<'a>,
        /// The pair's id.
        id: CorrelationId,
    },
    /// A seal was answered, or will never be: the end of its pair (DI-D-87).
    SealEnded {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// The lineage, by its place among the instance's lineages: the default is 0.
        lineage: u64,
        /// The epoch the seal is through, as its identity renders itself.
        through: fmt::Arguments<'a>,
        /// The pair's id.
        id: CorrelationId,
        /// How it ended.
        outcome: SealOutcome,
    },
    /// One part of a seal's flush was issued: the begin of its pair (DI-D-87).
    FlushBegan {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// The lineage, by its place among the instance's lineages: the default is 0.
        lineage: u64,
        /// The epoch the seal is through, as its identity renders itself.
        through: fmt::Arguments<'a>,
        /// Which part.
        part: FlushPart<'a>,
        /// The pair's id.
        id: CorrelationId,
    },
    /// One part of a seal's flush answered, or will never: the end of its pair (DI-D-87).
    FlushEnded {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// The lineage, by its place among the instance's lineages: the default is 0.
        lineage: u64,
        /// The epoch the seal is through, as its identity renders itself.
        through: fmt::Arguments<'a>,
        /// Which part.
        part: FlushPart<'a>,
        /// The pair's id.
        id: CorrelationId,
        /// How it ended.
        outcome: FlushOutcome,
    },
    /// An acquisition of one of an instance's locks found it held, and waits: the begin of its
    /// pair (DI-D-87).
    LockWaitBegan {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// Which lock: a name per lock.
        lock: &'static str,
        /// The pair's id.
        id: CorrelationId,
    },
    /// The waiting acquisition took the lock, and holds it: the end of its pair (DI-D-87).
    LockWaitEnded {
        /// The instance, by the number it was minted with.
        instance: u64,
        /// Which lock: a name per lock.
        lock: &'static str,
        /// The pair's id.
        id: CorrelationId,
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
            Self::SealBegan { .. } | Self::SealEnded { .. } => DiagnosticKind::Seal,
            Self::FlushBegan { .. } | Self::FlushEnded { .. } => DiagnosticKind::Flush,
            Self::LockWaitBegan { .. } | Self::LockWaitEnded { .. } => DiagnosticKind::LockWait,
        }
    }

    /// Which end of which pair the event is, or `None` for an event that is not one.
    #[must_use]
    pub fn pairing(&self) -> Option<Pairing> {
        let (id, edge) = match self {
            Self::SealBegan { id, .. }
            | Self::FlushBegan { id, .. }
            | Self::LockWaitBegan { id, .. } => (*id, Edge::Begin),
            Self::SealEnded { id, .. }
            | Self::FlushEnded { id, .. }
            | Self::LockWaitEnded { id, .. } => (*id, Edge::End),
            Self::Inconsistency { .. }
            | Self::LockWait { .. }
            | Self::FlushIssued { .. }
            | Self::FlushAnswered { .. }
            | Self::SealWaited { .. } => return None,
        };
        Some(Pairing {
            kind: self.kind(),
            id,
            edge,
        })
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
            Self::SealBegan {
                instance,
                lineage,
                through,
                id,
            } => write!(
                f,
                "instance {instance} lineage {lineage} seal through {through} (pair {id}): began"
            ),
            Self::SealEnded {
                instance,
                lineage,
                through,
                id,
                outcome,
            } => write!(
                f,
                "instance {instance} lineage {lineage} seal through {through} (pair {id}): {outcome}"
            ),
            Self::FlushBegan {
                instance,
                lineage,
                through,
                part,
                id,
            } => write!(
                f,
                "instance {instance} lineage {lineage} seal through {through}: flush of {part} \
                 (pair {id}) began"
            ),
            Self::FlushEnded {
                instance,
                lineage,
                through,
                part,
                id,
                outcome,
            } => write!(
                f,
                "instance {instance} lineage {lineage} seal through {through}: flush of {part} \
                 (pair {id}) {outcome}"
            ),
            Self::LockWaitBegan { instance, lock, id } => {
                write!(
                    f,
                    "instance {instance} (pair {id}): waits for its {lock} lock"
                )
            }
            Self::LockWaitEnded { instance, lock, id } => {
                write!(f, "instance {instance} (pair {id}): took its {lock} lock")
            }
        }
    }
}

/// Where an instance's diagnostics go (DI-D-51). Called from whatever thread observed the event,
/// with dioring's lock held, so a sink returns promptly and never calls back into the instance.
///
/// Its own synchronization is a leaf beneath dioring's (DI-D-80): a lock a sink takes is never
/// taken in a way that can wait on anything dioring's locks guard, and is never held by a thread
/// while it calls into an instance -- a consumer that holds a sink's lock around a call on the
/// instance included, since that call may report to the sink under the instance's lock. A lock
/// wait's end is reported with the lock just taken (DI-D-87), so a sink that broke this could
/// deadlock against the instance it reports on.
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
