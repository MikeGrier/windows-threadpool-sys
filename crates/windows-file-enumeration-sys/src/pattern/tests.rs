// Copyright (c) 2026 Mike Grier
//! Tests for the single-segment name matcher.

use super::*;

/// Build a pattern from tokens, keeping the tests readable.
fn pattern(tokens: Vec<PatternToken>) -> NamePattern {
    NamePattern::from_tokens(tokens)
}

fn literal(text: &str) -> PatternToken {
    PatternToken::Literal(Wtf16String::from(text))
}

fn matches(pattern: &NamePattern, name: &str) -> bool {
    pattern.matches(&Wtf16String::from(name), CaseSensitivity::Sensitive)
}

fn matches_ignoring_case(pattern: &NamePattern, name: &str) -> bool {
    pattern.matches(&Wtf16String::from(name), CaseSensitivity::Insensitive)
}

#[test]
fn a_literal_pattern_matches_only_that_name() {
    let subject = NamePattern::literal(&Wtf16String::from("readme.txt"));
    assert!(matches(&subject, "readme.txt"));
    assert!(!matches(&subject, "readme.txts"));
    assert!(!matches(&subject, "readme.tx"));
    assert!(!matches(&subject, "Readme.txt"));
}

#[test]
fn an_empty_pattern_matches_nothing_a_directory_can_contain() {
    let subject = NamePattern::empty();
    assert!(!matches(&subject, "a"));
    // It does match the empty name, which no directory entry ever has.
    assert!(subject.matches(&Wtf16String::new(), CaseSensitivity::Sensitive));
}

#[test]
fn any_run_matches_a_prefix_a_suffix_and_the_middle() {
    let suffix = pattern(vec![PatternToken::AnyRun, literal(".log")]);
    assert!(matches(&suffix, "server.log"));
    assert!(matches(&suffix, ".log"));
    assert!(!matches(&suffix, "server.log.gz"));

    let contains = pattern(vec![
        PatternToken::AnyRun,
        literal("err"),
        PatternToken::AnyRun,
    ]);
    assert!(matches(&contains, "an-error-here"));
    assert!(!matches(&contains, "all-fine"));
}

#[test]
fn any_run_matches_zero_code_points() {
    let subject = pattern(vec![literal("a"), PatternToken::AnyRun, literal("b")]);
    assert!(matches(&subject, "ab"));
    assert!(matches(&subject, "axxxb"));
}

#[test]
fn any_one_matches_exactly_one_code_point() {
    let subject = pattern(vec![literal("a"), PatternToken::AnyOne, literal("c")]);
    assert!(matches(&subject, "abc"));
    assert!(!matches(&subject, "ac"));
    assert!(!matches(&subject, "abbc"));
}

#[test]
fn any_one_consumes_a_whole_surrogate_pair() {
    // U+1F600 is one code point spelled as two code units, so a single AnyOne
    // must consume both rather than splitting the pair.
    let name: Vec<u16> = "a\u{1F600}c".encode_utf16().collect();
    assert_eq!(name.len(), 4);
    let subject = pattern(vec![literal("a"), PatternToken::AnyOne, literal("c")]);
    assert!(subject.matches(&Wtf16String::from_units(&name), CaseSensitivity::Sensitive));
}

#[test]
fn any_one_matches_an_unpaired_surrogate_as_one_code_point() {
    // A name a filesystem should not contain but might; it must still be
    // matchable rather than being silently unreachable.
    let name = [0x0061, 0xD800, 0x0063];
    let subject = pattern(vec![literal("a"), PatternToken::AnyOne, literal("c")]);
    assert!(subject.matches(&Wtf16String::from_units(&name), CaseSensitivity::Sensitive));
}

#[test]
fn any_run_does_not_split_a_surrogate_pair() {
    // Advancing by whole code points means no split can leave the trailing
    // literal starting mid-pair.
    let name: Vec<u16> = "\u{1F600}\u{1F600}z".encode_utf16().collect();
    let subject = pattern(vec![PatternToken::AnyRun, literal("z")]);
    assert!(subject.matches(&Wtf16String::from_units(&name), CaseSensitivity::Sensitive));
}

#[test]
fn alternation_tries_every_arm() {
    let subject = pattern(vec![
        PatternToken::AnyRun,
        PatternToken::Alternation(vec![
            NamePattern::literal(&Wtf16String::from(".log")),
            NamePattern::literal(&Wtf16String::from(".txt")),
        ]),
    ]);
    assert!(matches(&subject, "a.log"));
    assert!(matches(&subject, "a.txt"));
    assert!(!matches(&subject, "a.bin"));
}

#[test]
fn alternation_arms_compose_with_what_follows_them() {
    // The first arm can match at more than one length, so the arm and the rest
    // of the pattern have to be decided together.
    let subject = pattern(vec![
        PatternToken::Alternation(vec![
            NamePattern::from_tokens(vec![literal("a"), PatternToken::AnyRun]),
            NamePattern::literal(&Wtf16String::from("zzz")),
        ]),
        literal("end"),
    ]);
    assert!(matches(&subject, "aend"));
    assert!(matches(&subject, "axxend"));
    assert!(matches(&subject, "zzzend"));
    assert!(!matches(&subject, "bend"));
}

#[test]
fn an_empty_alternation_matches_nothing() {
    let subject = pattern(vec![PatternToken::Alternation(Vec::new())]);
    assert!(!matches(&subject, ""));
    assert!(!matches(&subject, "a"));
}

#[test]
fn insensitive_matching_uses_the_ordinal_uppercase_table() {
    let subject = NamePattern::literal(&Wtf16String::from("ReadMe.TXT"));
    assert!(matches_ignoring_case(&subject, "readme.txt"));
    assert!(matches_ignoring_case(&subject, "README.TXT"));
    assert!(!matches_ignoring_case(&subject, "readme.txtx"));
    assert!(!matches(&subject, "readme.txt"));
}

#[test]
fn insensitive_matching_works_inside_a_wildcard_pattern() {
    let subject = pattern(vec![PatternToken::AnyRun, literal(".LOG")]);
    assert!(matches_ignoring_case(&subject, "server.log"));
    assert!(!matches(&subject, "server.log"));
}

#[test]
fn insensitive_matching_folds_non_ascii_letters() {
    // The ordinal table covers more than ASCII, which is the point of using
    // Windows' own table rather than an ASCII fold.
    let subject = NamePattern::literal(&Wtf16String::from("\u{00E9}t\u{00E9}"));
    assert!(matches_ignoring_case(&subject, "\u{00C9}T\u{00C9}"));
}

#[test]
fn a_name_with_an_interior_nul_is_compared_as_content() {
    // Counted comparison means the NUL does not terminate the comparison, so
    // two names differing only after it are still distinguished.
    let left = [0x0061, 0x0000, 0x0062];
    let right = [0x0061, 0x0000, 0x0063];
    let subject = NamePattern::literal(&Wtf16String::from_units(&left));
    assert!(subject.matches(
        &Wtf16String::from_units(&left),
        CaseSensitivity::Insensitive
    ));
    assert!(!subject.matches(
        &Wtf16String::from_units(&right),
        CaseSensitivity::Insensitive
    ));
}

#[test]
fn a_pattern_can_be_built_by_chaining() {
    let subject = NamePattern::empty()
        .with(PatternToken::AnyRun)
        .with(literal(".rs"));
    assert_eq!(subject.tokens().len(), 2);
    assert!(matches(&subject, "lib.rs"));
}

#[test]
fn push_appends_in_order() {
    let mut subject = NamePattern::empty();
    subject.push(literal("a"));
    subject.push(PatternToken::AnyRun);
    assert!(matches(&subject, "abc"));
    assert!(!matches(&subject, "bac"));
}

#[test]
fn only_whole_names_match() {
    // There is no implicit anchoring to relax: a pattern always describes the
    // entire leaf name.
    let subject = NamePattern::literal(&Wtf16String::from("log"));
    assert!(!matches(&subject, "logs"));
    assert!(!matches(&subject, "catalog"));
}

#[test]
fn sensitive_is_the_default_case_rule() {
    assert_eq!(CaseSensitivity::default(), CaseSensitivity::Sensitive);
}
