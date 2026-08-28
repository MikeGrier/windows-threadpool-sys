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
//! Win32 call. [`CaseSensitivity::Insensitive`] uses `CompareStringOrdinal`,
//! Windows' own non-linguistic uppercase table -- the same notion of "equal
//! ignoring case" the filesystem uses, rather than a locale-dependent collation
//! that would make matching depend on the caller's user profile.
//!
//! Ordinal case folding is one-to-one per code unit, which is what lets a
//! literal run consume exactly its own length even when matched insensitively.

use windows_sys::Win32::Foundation::TRUE;
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
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
/// `CompareStringOrdinal` accepts explicit lengths, so an interior NUL in a name
/// is compared as content rather than terminating the comparison. An empty run
/// is handled here rather than passed on, because a zero length would otherwise
/// have to be distinguished from the API's own "NUL-terminated" convention.
fn ordinal_equal_ignoring_case(left: &[u16], right: &[u16]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    if left.is_empty() {
        return true;
    }
    let Ok(left_len) = i32::try_from(left.len()) else {
        // A run this long cannot be a filesystem name; fall back to the exact
        // comparison rather than truncating the length and comparing a prefix.
        return left == right;
    };
    let Ok(right_len) = i32::try_from(right.len()) else {
        return left == right;
    };
    // SAFETY: both pointers address `left_len`/`right_len` initialised code
    // units, which is exactly the counted form this API documents; the call
    // reads only that many units and writes nothing.
    let result =
        unsafe { CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, TRUE) };
    result == CSTR_EQUAL
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
