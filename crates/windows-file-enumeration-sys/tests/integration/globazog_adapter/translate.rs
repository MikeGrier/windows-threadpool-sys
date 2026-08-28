// Copyright (c) 2026 Mike Grier
//! Translating Globazog's predicate leaves into
//! `windows_file_enumeration_sys` predicate clauses, losslessly.
//!
//! # Why `IsType { ty: EntryType::Other, .. }` needs two clauses
//!
//! Windows' attribute model has exactly two entry kinds -- a bit is either
//! `FILE_ATTRIBUTE_DIRECTORY` or it is not -- so there is no windows-side
//! clause that means "neither file nor directory". A non-negated
//! `EntryType::Other` test must therefore translate into a query that can
//! never match on Windows, and a negated one into a query that always
//! matches (there being nothing here that ever *is* "other"). Requiring both
//! `FILE_ATTRIBUTE_DIRECTORY` set *and* clear is a contradiction no entry can
//! satisfy, which is exactly the "never matches" clause needed; negating the
//! whole leaf then makes it "always matches", both without inventing any
//! clause `windows_file_enumeration_sys` does not otherwise have.

use windows_file_enumeration_sys::{
    ComparisonOperator, EntryType, PatternToken, PredicateClause, QueryByExample,
    WindowsFileTimestamp,
};
use wtf_string::Wtf16String;

use crate::globazog_adapter::predicate_types::{
    CaseSensitivity, Cmp, Leaf, Segment, TimeField, Token,
};
use crate::globazog_adapter::types::{encode_codepoint_to_wtf16, unix_nanos_to_filetime_ticks};

/// `FILE_ATTRIBUTE_DIRECTORY`, used to build the self-contradictory
/// "never matches" clause pair for `EntryType::Other`.
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

fn translate_case(case: CaseSensitivity) -> windows_file_enumeration_sys::CaseSensitivity {
    match case {
        CaseSensitivity::Sensitive => windows_file_enumeration_sys::CaseSensitivity::Sensitive,
        CaseSensitivity::Insensitive => windows_file_enumeration_sys::CaseSensitivity::Insensitive,
    }
}

fn translate_cmp(cmp: Cmp) -> ComparisonOperator {
    match cmp {
        Cmp::Lt => ComparisonOperator::Less,
        Cmp::Le => ComparisonOperator::LessOrEqual,
        Cmp::Eq => ComparisonOperator::Equal,
        Cmp::Ne => ComparisonOperator::NotEqual,
        Cmp::Ge => ComparisonOperator::GreaterOrEqual,
        Cmp::Gt => ComparisonOperator::Greater,
    }
}

fn translate_time_field(field: TimeField) -> windows_file_enumeration_sys::TimestampField {
    match field {
        TimeField::Btime => windows_file_enumeration_sys::TimestampField::Creation,
        TimeField::Mtime => windows_file_enumeration_sys::TimestampField::LastWrite,
        TimeField::Atime => windows_file_enumeration_sys::TimestampField::LastAccess,
        TimeField::Ctime => windows_file_enumeration_sys::TimestampField::Change,
    }
}

fn translate_token(token: &Token) -> PatternToken {
    match token {
        Token::Literal(cp) => {
            PatternToken::Literal(Wtf16String::from_units(&encode_codepoint_to_wtf16(*cp)))
        }
        Token::Any => PatternToken::AnyOne,
        Token::Star => PatternToken::AnyRun,
        Token::Alt(segments) => {
            PatternToken::Alternation(segments.iter().map(translate_segment).collect())
        }
    }
}

/// Translate one compiled single-segment matcher into the equivalent
/// `windows_file_enumeration_sys` pattern.
#[must_use]
pub fn translate_segment(segment: &Segment) -> windows_file_enumeration_sys::NamePattern {
    windows_file_enumeration_sys::NamePattern::from_tokens(
        segment.iter().map(translate_token).collect(),
    )
}

/// Translate one Globazog predicate leaf into the equivalent
/// `windows_file_enumeration_sys` clause or clauses.
///
/// Every variant but `IsType { ty: EntryType::Other, .. }` translates to
/// exactly one clause; see the module doc comment for why that one variant
/// needs two.
#[must_use]
pub fn translate_leaf(leaf: &Leaf) -> Vec<PredicateClause> {
    match leaf {
        Leaf::Name { seg, case, negate } => vec![PredicateClause::Name {
            pattern: translate_segment(seg),
            case: translate_case(*case),
            negated: *negate,
        }],
        Leaf::NameInSet { segs, case, negate } => vec![PredicateClause::NameInSet {
            patterns: segs.iter().map(translate_segment).collect(),
            case: translate_case(*case),
            negated: *negate,
        }],
        Leaf::IsType {
            ty: crate::globazog_adapter::types::EntryType::File,
            negate,
        } => vec![PredicateClause::IsType {
            entry_type: EntryType::File,
            negated: *negate,
        }],
        Leaf::IsType {
            ty: crate::globazog_adapter::types::EntryType::Dir,
            negate,
        } => vec![PredicateClause::IsType {
            entry_type: EntryType::Directory,
            negated: *negate,
        }],
        Leaf::IsType {
            ty: crate::globazog_adapter::types::EntryType::Other,
            negate,
        } => {
            // See the module doc comment: this pair can never both hold, so
            // the conjunction never matches; `*negate` inverts the whole
            // leaf, not each clause individually, which is why it is applied
            // once here rather than to each of the two pushed clauses.
            let never_matches = [
                PredicateClause::AttributesAllSet(FILE_ATTRIBUTE_DIRECTORY),
                PredicateClause::AttributesAllClear(FILE_ATTRIBUTE_DIRECTORY),
            ];
            if *negate {
                // "always matches" has no single clause either; an empty
                // conjunction already matches everything, so the caller
                // simply omits this leaf. Returning no clauses here achieves
                // exactly that.
                Vec::new()
            } else {
                never_matches.to_vec()
            }
        }
        Leaf::IsReparse { negate } => vec![PredicateClause::IsReparsePoint { negated: *negate }],
        Leaf::ReparseTag { tag, negate } => vec![PredicateClause::ReparseTag {
            tag: *tag,
            negated: *negate,
        }],
        Leaf::AttrsAllSet(mask) => vec![PredicateClause::AttributesAllSet(*mask)],
        Leaf::AttrsAllClear(mask) => vec![PredicateClause::AttributesAllClear(*mask)],
        Leaf::Size { op, value } => vec![PredicateClause::LogicalSize {
            operator: translate_cmp(*op),
            value: *value,
        }],
        Leaf::Time { field, op, value } => vec![PredicateClause::Timestamp {
            field: translate_time_field(*field),
            operator: translate_cmp(*op),
            value: WindowsFileTimestamp::from_ticks(unix_nanos_to_filetime_ticks(*value)),
        }],
    }
}

/// Translate a whole conjunction of Globazog leaves into a
/// `windows_file_enumeration_sys` query.
///
/// # Panics
///
/// Panics if a translated clause is rejected as vacuous (a zero attribute
/// mask, or an empty name set) -- which would mean a leaf this function was
/// given was itself vacuous, a caller bug this adapter has no reason to hide.
#[must_use]
pub fn translate_leaves(leaves: &[Leaf]) -> QueryByExample {
    let mut query = QueryByExample::new();
    for leaf in leaves {
        for clause in translate_leaf(leaf) {
            query = query.with(clause).expect("a non-vacuous translated clause");
        }
    }
    query
}
