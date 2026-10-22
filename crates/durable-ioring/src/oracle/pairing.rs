// Copyright (c) 2026 Mike Grier
//! The pairing rule (DI-D-80, DI-D-87), as one executable definition: what a sink's stream of
//! begin and end events must satisfy. Every sink's tests bind to it -- the core's, the driver's,
//! and those of a sink outside this crate -- rather than restating the rule.
//!
//! Which events are the ends of which pairs is [`Diagnostic::pairing`]'s; this checks only how
//! they follow one another. An event that is no end of a pair is passed over.
//!
//! # Checked
//!
//! - **Every end follows a begin** of the same id, and of the same kind.
//! - **An end repeats its begin's identity**: the instance, and the lineage and epoch, the part, or
//!   the lock, as the pair has them.
//! - **No id is begun twice**, and **nothing of a pair follows its end**.
//! - **No begin is left open** when the stream is declared finished, except those the caller names,
//!   and each of those must be open.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::correlation::CorrelationId;
use crate::diagnostics::{Diagnostic, DiagnosticKind, Edge};

#[cfg(test)]
mod tests;

/// The pairing rule over a stream of events, fed one at a time in the order a sink received them.
#[derive(Debug, Default)]
pub struct PairingChecker {
    open: HashMap<CorrelationId, Begun>,
    ended: HashSet<CorrelationId>,
}

/// An open pair: its kind, and the identity its end must repeat.
#[derive(Debug)]
struct Begun {
    kind: DiagnosticKind,
    subject: String,
}

impl PairingChecker {
    /// A checker that has seen nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Check `event`, the stream's next, against what came before it.
    ///
    /// # Errors
    ///
    /// The [`PairingViolation`] it is, if it is one.
    pub fn observe(&mut self, event: &Diagnostic<'_>) -> Result<(), PairingViolation> {
        let Some(pairing) = event.pairing() else {
            return Ok(());
        };
        let id = pairing.id;
        if self.ended.contains(&id) {
            return Err(PairingViolation::AfterEnd {
                id,
                edge: pairing.edge,
            });
        }
        let subject = subject(event);
        match pairing.edge {
            Edge::Begin => {
                if self.open.contains_key(&id) {
                    return Err(PairingViolation::BegunTwice { id });
                }
                self.open.insert(
                    id,
                    Begun {
                        kind: pairing.kind,
                        subject,
                    },
                );
            }
            Edge::End => {
                let Some(begun) = self.open.remove(&id) else {
                    return Err(PairingViolation::EndWithoutBegin {
                        id,
                        kind: pairing.kind,
                    });
                };
                if begun.kind != pairing.kind {
                    return Err(PairingViolation::KindMismatch {
                        id,
                        began: begun.kind,
                        ended: pairing.kind,
                    });
                }
                if begun.subject != subject {
                    return Err(PairingViolation::SubjectMismatch {
                        id,
                        began: begun.subject,
                        ended: subject,
                    });
                }
                self.ended.insert(id);
            }
        }
        Ok(())
    }

    /// The pairs begun and not yet ended, by id, in order.
    #[must_use]
    pub fn open(&self) -> Vec<CorrelationId> {
        let mut open: Vec<CorrelationId> = self.open.keys().copied().collect();
        open.sort_unstable();
        open
    }

    /// Declare the stream finished, with exactly the pairs in `left_open` still open.
    ///
    /// # Errors
    ///
    /// [`PairingViolation::LeftOpen`] for the pairs open and not named, or
    /// [`PairingViolation::NamedNotOpen`] for a pair named and not open.
    pub fn finish(self, left_open: &[CorrelationId]) -> Result<(), PairingViolation> {
        let named: HashSet<CorrelationId> = left_open.iter().copied().collect();
        let mut unnamed: Vec<CorrelationId> = self
            .open
            .keys()
            .filter(|id| !named.contains(id))
            .copied()
            .collect();
        if !unnamed.is_empty() {
            unnamed.sort_unstable();
            return Err(PairingViolation::LeftOpen { ids: unnamed });
        }
        let mut not_open: Vec<CorrelationId> = named
            .into_iter()
            .filter(|id| !self.open.contains_key(id))
            .collect();
        not_open.sort_unstable();
        match not_open.first() {
            Some(&id) => Err(PairingViolation::NamedNotOpen { id }),
            None => Ok(()),
        }
    }
}

/// The identity an end repeats from its begin, as text: the instance, and what the pair is about.
fn subject(event: &Diagnostic<'_>) -> String {
    match event {
        Diagnostic::SealBegan {
            instance,
            lineage,
            through,
            ..
        }
        | Diagnostic::SealEnded {
            instance,
            lineage,
            through,
            ..
        } => format!("instance {instance} lineage {lineage} seal through {through}"),
        Diagnostic::FlushBegan {
            instance,
            lineage,
            through,
            part,
            ..
        }
        | Diagnostic::FlushEnded {
            instance,
            lineage,
            through,
            part,
            ..
        } => format!("instance {instance} lineage {lineage} seal through {through}, {part}"),
        Diagnostic::LockWaitBegan { instance, lock, .. }
        | Diagnostic::LockWaitEnded { instance, lock, .. } => {
            format!("instance {instance} lock {lock}")
        }
        _ => String::new(),
    }
}

/// How a stream broke the pairing rule.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PairingViolation {
    /// An end whose id was never begun.
    EndWithoutBegin {
        /// The id.
        id: CorrelationId,
        /// The end's kind.
        kind: DiagnosticKind,
    },
    /// A begin whose id is already open.
    BegunTwice {
        /// The id.
        id: CorrelationId,
    },
    /// An end of another kind than its begin.
    KindMismatch {
        /// The id.
        id: CorrelationId,
        /// The begin's kind.
        began: DiagnosticKind,
        /// The end's kind.
        ended: DiagnosticKind,
    },
    /// An end that does not repeat its begin's identity.
    SubjectMismatch {
        /// The id.
        id: CorrelationId,
        /// The begin's identity.
        began: String,
        /// The end's identity.
        ended: String,
    },
    /// An event of a pair that has already ended.
    AfterEnd {
        /// The id.
        id: CorrelationId,
        /// Which end the event is.
        edge: Edge,
    },
    /// Pairs open when the stream was declared finished, and not named as left open.
    LeftOpen {
        /// Their ids, in order.
        ids: Vec<CorrelationId>,
    },
    /// A pair named as left open that is not open: never begun, or ended.
    NamedNotOpen {
        /// The id.
        id: CorrelationId,
    },
}

impl fmt::Display for PairingViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndWithoutBegin { id, kind } => {
                write!(f, "pair {id} ({kind:?}) ended without having begun")
            }
            Self::BegunTwice { id } => write!(f, "pair {id} began while already open"),
            Self::KindMismatch { id, began, ended } => {
                write!(f, "pair {id} began as {began:?} and ended as {ended:?}")
            }
            Self::SubjectMismatch { id, began, ended } => {
                write!(f, "pair {id} began about {began} and ended about {ended}")
            }
            Self::AfterEnd { id, edge } => {
                write!(
                    f,
                    "pair {id} had ended, and an event of it ({edge:?}) followed"
                )
            }
            Self::LeftOpen { ids } => {
                let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
                write!(f, "pairs left open: {}", ids.join(", "))
            }
            Self::NamedNotOpen { id } => {
                write!(f, "pair {id} was named as left open, and is not open")
            }
        }
    }
}

impl std::error::Error for PairingViolation {}
