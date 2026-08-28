// Copyright (c) 2026 Mike Grier
//! What a receiver observes: entries, and exactly one terminal per enumeration.
//!
//! The completion ring carries two kinds of record and no more. In particular a
//! failure is *inside* its terminal rather than beside it, which is what makes
//! one reserved slot per accepted enumeration sufficient: reporting a failure
//! can never need room that a full ring does not have.

use crate::entry::DirectoryEntry;
use crate::error::EnumerationError;

/// Identifies one accepted enumeration within a session.
///
/// Every completion record carries one, because several enumerations may share
/// a session and their records interleave. Values are unique for the life of the
/// process, so an identifier retained past its enumeration names nothing rather
/// than aliasing a later one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnumerationId(u64);

impl EnumerationId {
    /// Reconstruct an identifier from a previously observed raw value.
    #[must_use]
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    /// The raw value, for logging or for carrying the identity through a
    /// caller's own data structures.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for EnumerationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "enumeration {}", self.0)
    }
}

/// How one enumeration ended.
///
/// Exactly one of these is delivered per accepted enumeration, after every entry
/// that enumeration produced -- with one deliberate exception: an enumeration
/// whose receiver has been dropped emits nothing, because no observer remains to
/// owe an outcome to.
#[derive(Debug)]
pub enum TerminalOutcome {
    /// The directory was enumerated to exhaustion.
    ///
    /// Every entry that satisfied the predicate has already been delivered.
    Completed,
    /// The enumeration stopped early because it was cancelled.
    ///
    /// Entries already queued when cancellation was observed are still
    /// delivered; cancellation discards only what had not yet been parsed.
    Cancelled,
    /// The enumeration stopped early because it failed.
    ///
    /// Entries delivered before the failure remain valid: a late failure
    /// truncates the listing rather than retracting it.
    Failed(EnumerationError),
}

impl TerminalOutcome {
    /// Whether this outcome is [`Completed`](Self::Completed).
    #[must_use]
    pub const fn is_completed(&self) -> bool {
        matches!(self, TerminalOutcome::Completed)
    }

    /// The failure, if this outcome is [`Failed`](Self::Failed).
    #[must_use]
    pub const fn failure(&self) -> Option<&EnumerationError> {
        match self {
            TerminalOutcome::Failed(error) => Some(error),
            _ => None,
        }
    }
}

/// One record taken from a session's completion ring.
#[derive(Debug)]
pub enum Completion {
    /// One directory entry that satisfied its request's predicate.
    Entry {
        /// The enumeration that produced it.
        enumeration: EnumerationId,
        /// The entry.
        entry: DirectoryEntry,
    },
    /// The single terminal outcome of one enumeration.
    ///
    /// No further record for that [`EnumerationId`] follows.
    Terminal {
        /// The enumeration that ended.
        enumeration: EnumerationId,
        /// How it ended.
        outcome: TerminalOutcome,
    },
}

impl Completion {
    /// The enumeration this record belongs to.
    #[must_use]
    pub const fn enumeration(&self) -> EnumerationId {
        match self {
            Completion::Entry { enumeration, .. } | Completion::Terminal { enumeration, .. } => {
                *enumeration
            }
        }
    }

    /// Whether this record ends its enumeration.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Completion::Terminal { .. })
    }
}

#[cfg(test)]
mod tests;
