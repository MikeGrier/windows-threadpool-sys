// Copyright (c) 2026 Mike Grier
//! The query-by-example predicate: what a request asks of each entry.
//!
//! A predicate is *data*, never a closure. That is what lets it cross the
//! submission ring, be validated before the enumeration is accepted, and run
//! inside a Windows thread-pool callback without this crate ever calling client
//! code on its cadence path. A caller-supplied closure would bring panics,
//! blocking, and reentrancy into a completion callback -- the one place none of
//! them can be handled.
//!
//! # Shape
//!
//! [`QueryByExample`] is a flat conjunction: an entry matches when every clause
//! matches. An empty query matches every entry the enumeration reaches.
//!
//! There is no explicit range clause because two comparison clauses over the
//! same field already are one, and no `OR` because
//! [`PatternToken::Alternation`](crate::PatternToken::Alternation) and
//! [`NameInSet`](PredicateClause::NameInSet) cover the disjunction that name
//! matching actually needs. Contradictory clauses are allowed and simply match
//! nothing.
//!
//! # Why vacuous clauses are rejected
//!
//! A zero attribute mask and an empty name set are both *silent* match-alls:
//! they look like a filter and behave like none. Both are rejected when the
//! query is built, where the caller can still see which clause was wrong.

use crate::entry::{DirectoryEntry, EntryType};
use crate::error::{PredicateError, PredicateFailure};
use crate::pattern::{CaseSensitivity, NamePattern};
use crate::timestamp::WindowsFileTimestamp;

/// How a numeric or timestamp clause compares.
///
/// The entry's value is always the left operand and the clause's value the
/// right, so `LogicalSize { operator: Greater, value: 4096 }` reads as "the
/// entry is larger than 4096 bytes".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComparisonOperator {
    /// `<`
    Less,
    /// `<=`
    LessOrEqual,
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `>=`
    GreaterOrEqual,
    /// `>`
    Greater,
}

impl ComparisonOperator {
    /// Apply this operator to an entry value and a clause value.
    fn apply<T: Ord>(self, entry: T, value: T) -> bool {
        match self {
            ComparisonOperator::Less => entry < value,
            ComparisonOperator::LessOrEqual => entry <= value,
            ComparisonOperator::Equal => entry == value,
            ComparisonOperator::NotEqual => entry != value,
            ComparisonOperator::GreaterOrEqual => entry >= value,
            ComparisonOperator::Greater => entry > value,
        }
    }
}

/// Which of an entry's four times a timestamp clause compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimestampField {
    /// [`DirectoryEntry::creation_time`]
    Creation,
    /// [`DirectoryEntry::last_access_time`]
    LastAccess,
    /// [`DirectoryEntry::last_write_time`]
    LastWrite,
    /// [`DirectoryEntry::change_time`]
    Change,
}

impl TimestampField {
    /// Read this field from an entry.
    fn read(self, entry: &DirectoryEntry) -> WindowsFileTimestamp {
        match self {
            TimestampField::Creation => entry.creation_time(),
            TimestampField::LastAccess => entry.last_access_time(),
            TimestampField::LastWrite => entry.last_write_time(),
            TimestampField::Change => entry.change_time(),
        }
    }
}

/// One condition an entry must satisfy.
///
/// Clauses that can be sensibly inverted carry their own `negated` flag rather
/// than relying on an enclosing `Not`, which keeps the query flat and keeps the
/// negation adjacent to the thing it negates.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PredicateClause {
    /// The name matches a pattern.
    Name {
        /// The pattern to match the entry's leaf name against.
        pattern: NamePattern,
        /// How the comparison treats case.
        case: CaseSensitivity,
        /// Invert the result.
        negated: bool,
    },
    /// The name matches any pattern in a non-empty set.
    NameInSet {
        /// The alternatives. Must not be empty.
        patterns: Vec<NamePattern>,
        /// How the comparisons treat case.
        case: CaseSensitivity,
        /// Invert the result, giving set non-membership.
        negated: bool,
    },
    /// The entry is (or, negated, is not) the given kind.
    IsType {
        /// The kind to test for.
        entry_type: EntryType,
        /// Invert the result.
        negated: bool,
    },
    /// The entry is (or, negated, is not) a reparse point.
    IsReparsePoint {
        /// Invert the result.
        negated: bool,
    },
    /// The entry is a reparse point whose tag equals `tag`.
    ///
    /// A non-reparse entry has no tag, so it never satisfies this clause --
    /// and, negated, always does.
    ReparseTag {
        /// The tag to compare against.
        tag: u32,
        /// Invert the result.
        negated: bool,
    },
    /// Every bit in the mask is set in the entry's attributes.
    ///
    /// The mask must be non-zero.
    AttributesAllSet(u32),
    /// Every bit in the mask is clear in the entry's attributes.
    ///
    /// The mask must be non-zero.
    AttributesAllClear(u32),
    /// The entry's logical size compares to `value` bytes.
    LogicalSize {
        /// How to compare.
        operator: ComparisonOperator,
        /// The size in bytes to compare against.
        value: u64,
    },
    /// The entry's allocation size compares to `value` bytes.
    AllocationSize {
        /// How to compare.
        operator: ComparisonOperator,
        /// The size in bytes to compare against.
        value: u64,
    },
    /// One of the entry's four times compares to `value`.
    Timestamp {
        /// Which time to read.
        field: TimestampField,
        /// How to compare.
        operator: ComparisonOperator,
        /// The timestamp to compare against.
        value: WindowsFileTimestamp,
    },
}

impl PredicateClause {
    /// Reject a clause that would silently match everything.
    fn validate(&self) -> Result<(), PredicateError> {
        match self {
            PredicateClause::AttributesAllSet(0) | PredicateClause::AttributesAllClear(0) => {
                Err(PredicateError::new(PredicateFailure::EmptyAttributeMask))
            }
            PredicateClause::NameInSet { patterns, .. } if patterns.is_empty() => {
                Err(PredicateError::new(PredicateFailure::EmptyNameSet))
            }
            _ => Ok(()),
        }
    }

    /// Whether `entry` satisfies this clause.
    #[must_use]
    pub fn matches(&self, entry: &DirectoryEntry) -> bool {
        match self {
            PredicateClause::Name {
                pattern,
                case,
                negated,
            } => pattern.matches(entry.name(), *case) != *negated,
            PredicateClause::NameInSet {
                patterns,
                case,
                negated,
            } => {
                let any = patterns
                    .iter()
                    .any(|pattern| pattern.matches(entry.name(), *case));
                any != *negated
            }
            PredicateClause::IsType {
                entry_type,
                negated,
            } => (entry.entry_type() == *entry_type) != *negated,
            PredicateClause::IsReparsePoint { negated } => entry.is_reparse_point() != *negated,
            PredicateClause::ReparseTag { tag, negated } => {
                (entry.reparse_tag() == Some(*tag)) != *negated
            }
            PredicateClause::AttributesAllSet(mask) => entry.attributes() & mask == *mask,
            PredicateClause::AttributesAllClear(mask) => entry.attributes() & mask == 0,
            PredicateClause::LogicalSize { operator, value } => {
                operator.apply(entry.logical_size(), *value)
            }
            PredicateClause::AllocationSize { operator, value } => {
                operator.apply(entry.allocation_size(), *value)
            }
            PredicateClause::Timestamp {
                field,
                operator,
                value,
            } => operator.apply(field.read(entry), *value),
        }
    }
}

/// A validated conjunction of clauses.
///
/// Every clause is checked as it is added, so a built query can never carry a
/// vacuous clause and evaluation has nothing left to validate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryByExample {
    clauses: Vec<PredicateClause>,
}

impl QueryByExample {
    /// An empty query, which matches every entry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one clause.
    ///
    /// # Errors
    ///
    /// Returns [`PredicateError`] if the clause would silently match every
    /// entry: a zero attribute mask, or an empty name set.
    pub fn push(&mut self, clause: PredicateClause) -> Result<(), PredicateError> {
        clause.validate()?;
        self.clauses.push(clause);
        Ok(())
    }

    /// Add one clause, taking and returning the query for chaining.
    ///
    /// # Errors
    ///
    /// As [`push`](Self::push).
    pub fn with(mut self, clause: PredicateClause) -> Result<Self, PredicateError> {
        self.push(clause)?;
        Ok(self)
    }

    /// The clauses, in the order they were added.
    #[must_use]
    pub fn clauses(&self) -> &[PredicateClause] {
        &self.clauses
    }

    /// Whether this query has no clauses, and so matches every entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clauses.is_empty()
    }

    /// Whether `entry` satisfies every clause.
    ///
    /// Short-circuits on the first clause that fails, so an early cheap clause
    /// spares the later expensive ones.
    #[must_use]
    pub fn matches(&self, entry: &DirectoryEntry) -> bool {
        self.clauses.iter().all(|clause| clause.matches(entry))
    }
}

/// What a request asks of each entry.
///
/// Deliberately an enum with a single variant today. The variant is the settled
/// v1 predicate; the enum is the seam that lets a later predicate family --
/// an expression tree, say -- be added without replacing the request API that
/// carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryPredicate {
    /// A flat conjunction of query-by-example clauses.
    QueryByExample(QueryByExample),
}

impl EntryPredicate {
    /// Whether `entry` satisfies this predicate.
    #[must_use]
    pub fn matches(&self, entry: &DirectoryEntry) -> bool {
        match self {
            EntryPredicate::QueryByExample(query) => query.matches(entry),
        }
    }

    /// Whether this predicate accepts every entry, so evaluation can be skipped
    /// entirely.
    #[must_use]
    pub fn matches_everything(&self) -> bool {
        match self {
            EntryPredicate::QueryByExample(query) => query.is_empty(),
        }
    }
}

impl Default for EntryPredicate {
    /// An empty query, which accepts every entry.
    fn default() -> Self {
        EntryPredicate::QueryByExample(QueryByExample::new())
    }
}

impl From<QueryByExample> for EntryPredicate {
    fn from(query: QueryByExample) -> Self {
        EntryPredicate::QueryByExample(query)
    }
}

#[cfg(test)]
mod tests;
