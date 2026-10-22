// Copyright (c) 2026 Mike Grier
//! Single-segment name patterns and the matcher that evaluates them.
//!
//! Patterns are *compiled data*, not a wildcard string handed to a filesystem.
//! That is a deliberate ownership choice: the crate specifies what a pattern
//! means, so the answer does not change with the volume, the redirector, or the
//! Windows version underneath. It also means a caller building a pattern
//! programmatically never has to escape anything.
//!
//! # What a code point is here
//!
//! [`PatternToken::AnyOne`] matches exactly one Unicode scalar. In WTF-16 that
//! is a valid surrogate pair (two code units) or a single non-surrogate unit --
//! and, because names may be ill-formed, a lone unpaired surrogate counts as one
//! as well. A pattern can therefore never split a valid pair, and can still
//! match a name a filesystem should not have allowed.
//!
//! # Case
//!
//! [`CaseSensitivity::Sensitive`] is exact code-unit comparison, which needs no
//! Win32 call. [`CaseSensitivity::Insensitive`] is `win-text-sys`'s
//! `compare_string_ordinal`, which is `CompareStringOrdinal`: Windows' own
//! non-linguistic uppercase table -- the same notion of "equal ignoring case"
//! the filesystem uses, rather than a locale-dependent collation that would
//! make matching depend on the caller's user profile.
//!
//! Ordinal case folding is one-to-one per code unit, which is what lets a
//! literal run consume exactly its own length even when matched insensitively.

use std::cmp::Ordering;

use win_text_sys::{Case, CompareStringOrdinalError, compare_string_ordinal};
use wtf_string::{Wtf16Str, Wtf16String};

/// How a name comparison treats case.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CaseSensitivity {
    /// Exact code-unit comparison.
    ///
    /// The default, because it is the only comparison that cannot surprise: it
    /// depends on nothing but the two values.
    #[default]
    Sensitive,
    /// Comparison through Windows' ordinal uppercase table.
    Insensitive,
}

/// One element of a [`NamePattern`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PatternToken {
    /// A run of literal code units, matched under the clause's case rule.
    Literal(Wtf16String),
    /// Exactly one code point.
    AnyOne,
    /// Zero or more code points.
    AnyRun,
    /// Any one of several alternative token sequences.
    ///
    /// An empty alternation matches nothing, which is a contradiction rather
    /// than a vacuous match-all, so it needs no validation.
    Alternation(Vec<NamePattern>),
}

/// A compiled pattern for one leaf name.
///
/// A pattern never spans path separators because the value it is matched
/// against is a single directory entry's own name, which contains none. There
/// is consequently no "crosses a segment" case to specify.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamePattern {
    tokens: Vec<PatternToken>,
}

impl NamePattern {
    /// A pattern matching only the empty name.
    ///
    /// A directory entry's name is never empty, so this matches nothing. It is
    /// the identity a builder starts from, not a useful clause on its own.
    #[must_use]
    pub fn empty() -> Self {
        Self { tokens: Vec::new() }
    }

    /// A pattern that matches exactly `name`.
    #[must_use]
    pub fn literal(name: &Wtf16Str) -> Self {
        Self {
            tokens: vec![PatternToken::Literal(Wtf16String::from_units(
                name.as_units(),
            ))],
        }
    }

    /// A pattern built from an explicit token sequence.
    #[must_use]
    pub fn from_tokens(tokens: Vec<PatternToken>) -> Self {
        Self { tokens }
    }

    /// Append one token.
    pub fn push(&mut self, token: PatternToken) {
        self.tokens.push(token);
    }

    /// Append one token, taking and returning the pattern for chaining.
    #[must_use]
    pub fn with(mut self, token: PatternToken) -> Self {
        self.tokens.push(token);
        self
    }

    /// The pattern's tokens, in order.
    #[must_use]
    pub fn tokens(&self) -> &[PatternToken] {
        &self.tokens
    }

    /// Whether this pattern matches `name` in its entirety.
    #[must_use]
    pub fn matches(&self, name: &Wtf16Str, case: CaseSensitivity) -> bool {
        match_tokens(&self.tokens, name.as_units(), case)
    }
}

/// Whether `tokens` match all of `units`.
///
/// Backtracking rather than table-driven: a leaf name is short, the token count
/// is caller-chosen and small, and a matcher that is obviously correct is worth
/// more here than one that is asymptotically better on inputs this API never
/// sees.
fn match_tokens(tokens: &[PatternToken], units: &[u16], case: CaseSensitivity) -> bool {
    let Some((first, rest)) = tokens.split_first() else {
        return units.is_empty();
    };

    match first {
        PatternToken::Literal(literal) => {
            let width = literal.len();
            match_literal_prefix(units, literal, case) && match_tokens(rest, &units[width..], case)
        }
        PatternToken::AnyOne => match code_point_width(units) {
            Some(width) => match_tokens(rest, &units[width..], case),
            None => false,
        },
        // Try the shortest consumption first and grow. Advancing by whole code
        // points rather than code units is what keeps a valid surrogate pair
        // from being split across the boundary between this token and the next.
        PatternToken::AnyRun => {
            let mut remaining = units;
            loop {
                if match_tokens(rest, remaining, case) {
                    return true;
                }
                match code_point_width(remaining) {
                    Some(width) => remaining = &remaining[width..],
                    None => return false,
                }
            }
        }
        PatternToken::Alternation(arms) => arms.iter().any(|arm| {
            // Each arm must be matched jointly with what follows it, so an arm
            // that could match several lengths still lets the rest of the
            // pattern decide. Splicing is the simplest way to express that
            // without a continuation-passing matcher.
            let mut spliced = arm.tokens.clone();
            spliced.extend_from_slice(rest);
            match_tokens(&spliced, units, case)
        }),
    }
}

/// Whether `units` starts with `literal` under `case`.
fn match_literal_prefix(units: &[u16], literal: &Wtf16Str, case: CaseSensitivity) -> bool {
    let width = literal.len();
    if units.len() < width {
        return false;
    }
    units_equal(&units[..width], literal.as_units(), case)
}

/// Whether two equal-length code-unit runs are equal under `case`.
fn units_equal(left: &[u16], right: &[u16], case: CaseSensitivity) -> bool {
    match case {
        CaseSensitivity::Sensitive => left == right,
        CaseSensitivity::Insensitive => ordinal_equal_ignoring_case(left, right),
    }
}

/// Whether two runs are equal through Windows' ordinal uppercase table.
///
/// The comparison is given each run with its length, so an interior NUL in a
/// name is compared as content rather than terminating the comparison, and an
/// empty run is an ordinary comparison.
fn ordinal_equal_ignoring_case(left: &[u16], right: &[u16]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    equal_from(
        compare_string_ordinal(left, right, Case::Insensitive),
        left,
        right,
    )
}

/// What a comparison's result says about whether `left` and `right` are equal.
///
/// A failure of the call is not equality. A run too long for the call's count
/// cannot be a filesystem name, and is compared exactly rather than truncated
/// to a prefix; reaching it takes a slice of more than 2^31 UTF-16 units, over
/// four gigabytes of name, which no unit test should allocate, so it is tested
/// here through the result and not through a slice that long.
fn equal_from(
    result: Result<Ordering, CompareStringOrdinalError>,
    left: &[u16],
    right: &[u16],
) -> bool {
    match result {
        Ok(ordering) => ordering == Ordering::Equal,
        Err(CompareStringOrdinalError::TooLong { .. }) => left == right,
        Err(CompareStringOrdinalError::Call(_)) => false,
    }
}

/// How many code units the first code point of `units` occupies, or `None` when
/// `units` is empty.
///
/// A high surrogate followed by a low surrogate is one code point of two units.
/// Anything else -- including an unpaired surrogate, which a WTF-16 name may
/// legitimately contain -- is one unit.
fn code_point_width(units: &[u16]) -> Option<usize> {
    let first = *units.first()?;
    let is_high_surrogate = (0xD800..0xDC00).contains(&first);
    let has_low_surrogate = units
        .get(1)
        .is_some_and(|second| (0xDC00..0xE000).contains(second));
    Some(if is_high_surrogate && has_low_surrogate {
        2
    } else {
        1
    })
}

#[cfg(test)]
mod tests;
