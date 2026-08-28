// Copyright (c) 2026 Mike Grier
//! Globazog's predicate vocabulary, reconstructed for translation testing.
//!
//! # Provenance
//!
//! Reconstructed from `MikeGrier/globazog-rs` at
//! `55a0b1aec7a93051a675852636ab41a6437440fb`:
//! `crates/globazog/src/predicate.rs` (`Leaf`, `Cmp`, `TimeField`) and
//! `crates/globazog/src/syntax.rs` (`CaseSensitivity`, `Token`, `Segment`).
//! See [`types`](super::types) for the equivalent note on `DirEntry` and its
//! neighbors.
//!
//! `Leaf::Depth` is deliberately not reconstructed here: it compares an
//! entry's distance from a traversal root, which is a property of Globazog's
//! own recursive-traversal engine composing many single-directory requests,
//! never a property one directory's own listing can answer. A one-directory
//! backend -- what this adapter replaces -- has no depth to translate.

use crate::globazog_adapter::types::CodePoint;

/// Whether matching folds case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseSensitivity {
    /// Exact code-point comparison.
    Sensitive,
    /// Case-folded comparison via the Windows ordinal uppercase table.
    Insensitive,
}

/// One token within a single path segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// A literal code point.
    Literal(CodePoint),
    /// `?` -- exactly one code point.
    Any,
    /// `*` -- zero or more code points within the segment.
    Star,
    /// `{a,b,...}` -- n-ary alternation; each arm is a token sequence.
    Alt(Vec<Segment>),
}

/// A single path segment's matcher: a sequence of [`Token`]s.
pub type Segment = Vec<Token>;

/// A comparison operator for numeric [`Leaf`] conditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `>=`
    Ge,
    /// `>`
    Gt,
}

/// Which timestamp a [`Leaf::Time`] condition compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeField {
    /// Birth / creation time.
    Btime,
    /// Last-modification time.
    Mtime,
    /// Last-access time.
    Atime,
    /// Metadata-change time.
    Ctime,
}

/// One signed condition over an entry's metadata.
#[derive(Clone, Debug)]
pub enum Leaf {
    /// The name matches a single-segment glob; `negate` inverts the result.
    Name {
        /// The single-segment matcher.
        seg: Segment,
        /// Case rule for the comparison.
        case: CaseSensitivity,
        /// Invert the match.
        negate: bool,
    },
    /// The name matches any glob in the set; `negate` inverts (set
    /// non-membership).
    NameInSet {
        /// The alternatives to match against.
        segs: Vec<Segment>,
        /// Case rule for the comparison.
        case: CaseSensitivity,
        /// Invert to set non-membership.
        negate: bool,
    },
    /// The entry is (or, negated, is not) the given type.
    IsType {
        /// The type to test.
        ty: crate::globazog_adapter::types::EntryType,
        /// Invert the test.
        negate: bool,
    },
    /// The entry is (or, negated, is not) a reparse point.
    IsReparse {
        /// Invert the test.
        negate: bool,
    },
    /// The reparse tag equals (or, negated, differs from) `tag`.
    ReparseTag {
        /// The tag to compare.
        tag: u32,
        /// Invert the test.
        negate: bool,
    },
    /// Every bit in the mask is set in the attributes.
    AttrsAllSet(u32),
    /// Every bit in the mask is clear in the attributes.
    AttrsAllClear(u32),
    /// The size compares to `value` via `op`.
    Size {
        /// The comparison operator.
        op: Cmp,
        /// The size to compare against, in bytes.
        value: u64,
    },
    /// The `field` timestamp compares to `value` (Unix nanoseconds) via `op`.
    Time {
        /// Which timestamp to compare.
        field: TimeField,
        /// The comparison operator.
        op: Cmp,
        /// The timestamp to compare against, in nanoseconds since the Unix
        /// epoch.
        value: i64,
    },
}
