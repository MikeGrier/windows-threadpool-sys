// Copyright (c) 2026 Mike Grier
//! Tests for the query-by-example predicate.

use super::*;
use crate::error::PredicateFailure;
use crate::testing::{
    ATTR_DIRECTORY, ATTR_HIDDEN, ATTR_READONLY, EntryBuilder, named_directory, named_file,
};
use wtf_string::Wtf16String;

fn name_clause(text: &str, negated: bool) -> PredicateClause {
    PredicateClause::Name {
        pattern: NamePattern::literal(&Wtf16String::from(text)),
        case: CaseSensitivity::Sensitive,
        negated,
    }
}

fn query(clauses: Vec<PredicateClause>) -> QueryByExample {
    let mut built = QueryByExample::new();
    for clause in clauses {
        built.push(clause).expect("the clause is valid");
    }
    built
}

#[test]
fn an_empty_query_matches_every_entry() {
    let subject = QueryByExample::new();
    assert!(subject.is_empty());
    assert!(subject.matches(&named_file("anything")));
    assert!(subject.matches(&named_directory("anything")));
}

#[test]
fn the_default_predicate_accepts_everything() {
    let subject = EntryPredicate::default();
    assert!(subject.matches_everything());
    assert!(subject.matches(&named_file("x")));
}

#[test]
fn a_non_empty_query_no_longer_accepts_everything() {
    let subject = EntryPredicate::from(query(vec![name_clause("a", false)]));
    assert!(!subject.matches_everything());
}

#[test]
fn clauses_are_conjoined() {
    let subject = query(vec![
        name_clause("report.txt", false),
        PredicateClause::IsType {
            entry_type: EntryType::File,
            negated: false,
        },
    ]);
    assert!(subject.matches(&named_file("report.txt")));
    // Right name, wrong type.
    assert!(!subject.matches(&named_directory("report.txt")));
    // Right type, wrong name.
    assert!(!subject.matches(&named_file("other.txt")));
}

#[test]
fn contradictory_clauses_simply_match_nothing() {
    let subject = query(vec![name_clause("a", false), name_clause("b", false)]);
    assert!(!subject.matches(&named_file("a")));
    assert!(!subject.matches(&named_file("b")));
}

#[test]
fn a_name_clause_can_be_negated() {
    let subject = query(vec![name_clause("skip-me", true)]);
    assert!(subject.matches(&named_file("keep-me")));
    assert!(!subject.matches(&named_file("skip-me")));
}

#[test]
fn a_name_set_matches_any_member() {
    let subject = query(vec![PredicateClause::NameInSet {
        patterns: vec![
            NamePattern::literal(&Wtf16String::from("a.txt")),
            NamePattern::literal(&Wtf16String::from("b.txt")),
        ],
        case: CaseSensitivity::Sensitive,
        negated: false,
    }]);
    assert!(subject.matches(&named_file("a.txt")));
    assert!(subject.matches(&named_file("b.txt")));
    assert!(!subject.matches(&named_file("c.txt")));
}

#[test]
fn a_negated_name_set_is_non_membership() {
    let subject = query(vec![PredicateClause::NameInSet {
        patterns: vec![NamePattern::literal(&Wtf16String::from("a.txt"))],
        case: CaseSensitivity::Sensitive,
        negated: true,
    }]);
    assert!(!subject.matches(&named_file("a.txt")));
    assert!(subject.matches(&named_file("b.txt")));
}

#[test]
fn an_empty_name_set_is_rejected_when_the_query_is_built() {
    // Positively it matches nothing and negated it matches everything, so it is
    // a filter that silently is not one.
    let error = QueryByExample::new()
        .push(PredicateClause::NameInSet {
            patterns: Vec::new(),
            case: CaseSensitivity::Sensitive,
            negated: false,
        })
        .expect_err("an empty name set is vacuous");
    assert_eq!(error.failure(), PredicateFailure::EmptyNameSet);
}

#[test]
fn a_type_clause_can_be_negated() {
    let subject = query(vec![PredicateClause::IsType {
        entry_type: EntryType::Directory,
        negated: true,
    }]);
    assert!(subject.matches(&named_file("x")));
    assert!(!subject.matches(&named_directory("x")));
}

#[test]
fn reparse_status_and_tag_are_independently_testable() {
    let link = EntryBuilder::file("link").reparse(0xA000_000C).build();
    let plain = named_file("plain");

    let is_reparse = query(vec![PredicateClause::IsReparsePoint { negated: false }]);
    assert!(is_reparse.matches(&link));
    assert!(!is_reparse.matches(&plain));

    let has_tag = query(vec![PredicateClause::ReparseTag {
        tag: 0xA000_000C,
        negated: false,
    }]);
    assert!(has_tag.matches(&link));
    assert!(!has_tag.matches(&plain));

    let other_tag = query(vec![PredicateClause::ReparseTag {
        tag: 0xA000_0003,
        negated: false,
    }]);
    assert!(!other_tag.matches(&link));
}

#[test]
fn a_negated_tag_clause_accepts_an_entry_with_no_tag() {
    let subject = query(vec![PredicateClause::ReparseTag {
        tag: 0xA000_000C,
        negated: true,
    }]);
    assert!(subject.matches(&named_file("plain")));
}

#[test]
fn attribute_masks_test_all_bits_together() {
    let entry = EntryBuilder::file("x")
        .attributes(ATTR_READONLY | ATTR_HIDDEN)
        .build();

    let both_set = query(vec![PredicateClause::AttributesAllSet(
        ATTR_READONLY | ATTR_HIDDEN,
    )]);
    assert!(both_set.matches(&entry));

    // One of the two bits is missing, so "all set" fails.
    let with_directory = query(vec![PredicateClause::AttributesAllSet(
        ATTR_READONLY | ATTR_DIRECTORY,
    )]);
    assert!(!with_directory.matches(&entry));

    let none_set = query(vec![PredicateClause::AttributesAllClear(ATTR_DIRECTORY)]);
    assert!(none_set.matches(&entry));

    let clear_of_a_set_bit = query(vec![PredicateClause::AttributesAllClear(
        ATTR_READONLY | ATTR_DIRECTORY,
    )]);
    assert!(!clear_of_a_set_bit.matches(&entry));
}

#[test]
fn a_zero_attribute_mask_is_rejected_either_way_round() {
    for clause in [
        PredicateClause::AttributesAllSet(0),
        PredicateClause::AttributesAllClear(0),
    ] {
        let error = QueryByExample::new()
            .push(clause)
            .expect_err("a zero mask is vacuous");
        assert_eq!(error.failure(), PredicateFailure::EmptyAttributeMask);
    }
}

#[test]
fn every_comparison_operator_compares_the_entry_on_the_left() {
    let entry = EntryBuilder::file("x").logical_size(100).build();
    let cases = [
        (ComparisonOperator::Less, 101, true),
        (ComparisonOperator::Less, 100, false),
        (ComparisonOperator::LessOrEqual, 100, true),
        (ComparisonOperator::LessOrEqual, 99, false),
        (ComparisonOperator::Equal, 100, true),
        (ComparisonOperator::Equal, 99, false),
        (ComparisonOperator::NotEqual, 99, true),
        (ComparisonOperator::NotEqual, 100, false),
        (ComparisonOperator::GreaterOrEqual, 100, true),
        (ComparisonOperator::GreaterOrEqual, 101, false),
        (ComparisonOperator::Greater, 99, true),
        (ComparisonOperator::Greater, 100, false),
    ];
    for (operator, value, expected) in cases {
        let subject = query(vec![PredicateClause::LogicalSize { operator, value }]);
        assert_eq!(
            subject.matches(&entry),
            expected,
            "{operator:?} against {value}"
        );
    }
}

#[test]
fn logical_and_allocation_sizes_are_separate_fields() {
    let entry = EntryBuilder::file("sparse")
        .logical_size(1_000_000)
        .allocation_size(4096)
        .build();
    let big_logical = query(vec![PredicateClause::LogicalSize {
        operator: ComparisonOperator::Greater,
        value: 500_000,
    }]);
    let small_allocation = query(vec![PredicateClause::AllocationSize {
        operator: ComparisonOperator::LessOrEqual,
        value: 4096,
    }]);
    assert!(big_logical.matches(&entry));
    assert!(small_allocation.matches(&entry));
}

#[test]
fn each_timestamp_field_reads_its_own_value() {
    let entry = EntryBuilder::file("x").times(10, 20, 30, 40).build();
    let cases = [
        (TimestampField::Creation, 10),
        (TimestampField::LastAccess, 20),
        (TimestampField::LastWrite, 30),
        (TimestampField::Change, 40),
    ];
    for (field, ticks) in cases {
        let subject = query(vec![PredicateClause::Timestamp {
            field,
            operator: ComparisonOperator::Equal,
            value: WindowsFileTimestamp::from_ticks(ticks),
        }]);
        assert!(subject.matches(&entry), "{field:?} should equal {ticks}");
    }
}

#[test]
fn two_comparisons_over_one_field_express_a_range() {
    let subject = query(vec![
        PredicateClause::LogicalSize {
            operator: ComparisonOperator::GreaterOrEqual,
            value: 1024,
        },
        PredicateClause::LogicalSize {
            operator: ComparisonOperator::Less,
            value: 4096,
        },
    ]);
    assert!(subject.matches(&EntryBuilder::file("x").logical_size(2048).build()));
    assert!(!subject.matches(&EntryBuilder::file("x").logical_size(512).build()));
    assert!(!subject.matches(&EntryBuilder::file("x").logical_size(8192).build()));
}

#[test]
fn a_sentinel_timestamp_participates_as_its_raw_value() {
    // A filesystem that does not track change time reports zero, which compares
    // as less than every real time rather than being excluded from comparison.
    let entry = EntryBuilder::file("x").times(100, 100, 100, 0).build();
    let subject = query(vec![PredicateClause::Timestamp {
        field: TimestampField::Change,
        operator: ComparisonOperator::Less,
        value: WindowsFileTimestamp::from_ticks(50),
    }]);
    assert!(subject.matches(&entry));
}

#[test]
fn clauses_are_kept_in_the_order_they_were_added() {
    let subject = query(vec![
        name_clause("a", false),
        PredicateClause::AttributesAllSet(ATTR_READONLY),
    ]);
    assert_eq!(subject.clauses().len(), 2);
    assert!(matches!(subject.clauses()[0], PredicateClause::Name { .. }));
    assert!(matches!(
        subject.clauses()[1],
        PredicateClause::AttributesAllSet(_)
    ));
}

#[test]
fn a_rejected_clause_does_not_join_the_query() {
    let mut subject = QueryByExample::new();
    subject.push(name_clause("a", false)).expect("valid");
    subject
        .push(PredicateClause::AttributesAllSet(0))
        .expect_err("vacuous");
    assert_eq!(subject.clauses().len(), 1);
}

#[test]
fn the_chaining_builder_reports_the_first_invalid_clause() {
    let error = QueryByExample::new()
        .with(name_clause("a", false))
        .expect("valid")
        .with(PredicateClause::AttributesAllClear(0))
        .expect_err("vacuous");
    assert_eq!(error.failure(), PredicateFailure::EmptyAttributeMask);
}
