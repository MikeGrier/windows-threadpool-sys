// Copyright (c) 2026 Mike Grier
//! Every predicate operator and case mode, evaluated against real entries
//! with known sizes, names, and attributes.

use windows_file_enumeration_sys::{
    CaseSensitivity, ComparisonOperator, EntryPredicate, EntryType, EnumerationRequest,
    NamePattern, PatternToken, PredicateClause, QueryByExample, Session, TimestampField,
    WindowsFileTimestamp,
};
use wtf_string::Wtf16String;

use crate::support::{Scratch, drain_to_terminal, entry_names};

fn run(scratch: &Scratch, predicate: QueryByExample) -> Vec<String> {
    let request = EnumerationRequest::for_path(scratch.path())
        .expect("resolvable")
        .with_predicate(EntryPredicate::from(predicate));
    let (session, receiver) = Session::new(8, 8).expect("room");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();
    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    entry_names(&entries)
}

fn scratch_with_sized_files() -> Scratch {
    let scratch = Scratch::empty();
    std::fs::write(scratch.child("small.dat"), vec![0u8; 10]).expect("a file");
    std::fs::write(scratch.child("medium.dat"), vec![0u8; 100]).expect("a file");
    std::fs::write(scratch.child("large.dat"), vec![0u8; 1000]).expect("a file");
    scratch
}

#[test]
fn every_logical_size_comparison_operator_selects_the_right_files() {
    let scratch = scratch_with_sized_files();

    let cases: &[(ComparisonOperator, u64, &[&str])] = &[
        (ComparisonOperator::Less, 100, &["small.dat"]),
        (
            ComparisonOperator::LessOrEqual,
            100,
            &["small.dat", "medium.dat"],
        ),
        (ComparisonOperator::Equal, 100, &["medium.dat"]),
        (
            ComparisonOperator::NotEqual,
            100,
            &["small.dat", "large.dat"],
        ),
        (
            ComparisonOperator::GreaterOrEqual,
            100,
            &["medium.dat", "large.dat"],
        ),
        (ComparisonOperator::Greater, 100, &["large.dat"]),
    ];

    for (operator, value, expected) in cases {
        let query = QueryByExample::new()
            .with(PredicateClause::LogicalSize {
                operator: *operator,
                value: *value,
            })
            .expect("a non-vacuous clause");
        let mut delivered = run(&scratch, query);
        delivered.sort();
        let mut expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        assert_eq!(delivered, expected, "{operator:?}");
    }
}

#[test]
fn both_case_modes_agree_or_disagree_exactly_as_documented() {
    let scratch = Scratch::with_files(&["Report.TXT"]);

    let sensitive = QueryByExample::new()
        .with(PredicateClause::Name {
            pattern: NamePattern::literal(&Wtf16String::from("report.txt")),
            case: CaseSensitivity::Sensitive,
            negated: false,
        })
        .expect("a non-vacuous clause");
    assert!(
        run(&scratch, sensitive).is_empty(),
        "exact code-unit comparison must not match differing case"
    );

    let insensitive = QueryByExample::new()
        .with(PredicateClause::Name {
            pattern: NamePattern::literal(&Wtf16String::from("report.txt")),
            case: CaseSensitivity::Insensitive,
            negated: false,
        })
        .expect("a non-vacuous clause");
    assert_eq!(run(&scratch, insensitive), ["Report.TXT"]);
}

#[test]
fn is_type_distinguishes_files_from_directories() {
    let scratch = Scratch::with_files(&["a-file.txt"]);
    scratch.subdir("a-directory");

    let files_only = QueryByExample::new()
        .with(PredicateClause::IsType {
            entry_type: EntryType::File,
            negated: false,
        })
        .expect("a non-vacuous clause");
    assert_eq!(run(&scratch, files_only), ["a-file.txt"]);

    let directories_only = QueryByExample::new()
        .with(PredicateClause::IsType {
            entry_type: EntryType::Directory,
            negated: false,
        })
        .expect("a non-vacuous clause");
    assert_eq!(run(&scratch, directories_only), ["a-directory"]);
}

#[test]
fn name_in_set_matches_any_alternative() {
    let scratch = Scratch::with_files(&["one.txt", "two.txt", "three.txt"]);
    let query = QueryByExample::new()
        .with(PredicateClause::NameInSet {
            patterns: vec![
                NamePattern::literal(&Wtf16String::from("one.txt")),
                NamePattern::literal(&Wtf16String::from("three.txt")),
            ],
            case: CaseSensitivity::Sensitive,
            negated: false,
        })
        .expect("a non-vacuous clause");
    let mut delivered = run(&scratch, query);
    delivered.sort();
    assert_eq!(delivered, ["one.txt", "three.txt"]);
}

#[test]
fn a_wildcard_pattern_matches_a_run_of_any_length() {
    let scratch = Scratch::with_files(&["report-2026.log", "report.log", "other.txt"]);
    let pattern = NamePattern::empty()
        .with(PatternToken::Literal(Wtf16String::from("report")))
        .with(PatternToken::AnyRun)
        .with(PatternToken::Literal(Wtf16String::from(".log")));
    let query = QueryByExample::new()
        .with(PredicateClause::Name {
            pattern,
            case: CaseSensitivity::Sensitive,
            negated: false,
        })
        .expect("a non-vacuous clause");
    let mut delivered = run(&scratch, query);
    delivered.sort();
    assert_eq!(delivered, ["report-2026.log", "report.log"]);
}

#[test]
fn attribute_mask_clauses_select_by_read_only_status() {
    let scratch = Scratch::with_files(&["writable.txt", "readonly.txt"]);
    let readonly_path = scratch.child("readonly.txt");
    let mut permissions = std::fs::metadata(&readonly_path)
        .expect("metadata")
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&readonly_path, permissions).expect("set readonly");

    const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;

    let all_set = QueryByExample::new()
        .with(PredicateClause::AttributesAllSet(FILE_ATTRIBUTE_READONLY))
        .expect("a non-vacuous clause");
    assert_eq!(run(&scratch, all_set), ["readonly.txt"]);

    let all_clear = QueryByExample::new()
        .with(PredicateClause::AttributesAllClear(FILE_ATTRIBUTE_READONLY))
        .expect("a non-vacuous clause");
    assert_eq!(run(&scratch, all_clear), ["writable.txt"]);

    // Undo, so `Scratch::drop`'s recursive delete is not blocked by it. This
    // crate is Windows-only (`#![cfg(windows)]`); clippy's Unix
    // world-writable concern for `set_readonly(false)` does not apply here.
    let mut permissions = std::fs::metadata(&readonly_path)
        .expect("metadata")
        .permissions();
    #[allow(
        clippy::permissions_set_readonly_false,
        reason = "Windows-only; there is no Unix world-writable bit to worry about"
    )]
    permissions.set_readonly(false);
    std::fs::set_permissions(&readonly_path, permissions).expect("clear readonly");
}

#[test]
fn is_reparse_point_negated_matches_ordinary_entries() {
    let scratch = Scratch::with_files(&["ordinary.txt"]);
    let query = QueryByExample::new()
        .with(PredicateClause::IsReparsePoint { negated: true })
        .expect("a non-vacuous clause");
    assert_eq!(run(&scratch, query), ["ordinary.txt"]);
}

#[test]
fn every_timestamp_field_is_readable_through_its_own_clause() {
    let scratch = Scratch::with_files(&["timed.txt"]);
    for field in [
        TimestampField::Creation,
        TimestampField::LastAccess,
        TimestampField::LastWrite,
        TimestampField::Change,
    ] {
        let query = QueryByExample::new()
            .with(PredicateClause::Timestamp {
                field,
                operator: ComparisonOperator::GreaterOrEqual,
                value: WindowsFileTimestamp::ZERO,
            })
            .expect("a non-vacuous clause");
        assert_eq!(run(&scratch, query), ["timed.txt"], "{field:?}");
    }
}
