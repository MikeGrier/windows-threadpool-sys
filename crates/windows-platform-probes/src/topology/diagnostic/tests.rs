// Copyright (c) Mike Grier.

//! Tests for what the diagnostics publish into the row.
//!
//! # Why these are literals
//!
//! Every assertion here writes the expected wire form out by hand. That is
//! deliberate, and it is the opposite of what this crate does elsewhere: a rule
//! that is a PREDICATE over values is defined once and asked, never restated,
//! because a hand-written second copy checks the copy rather than the contract.
//!
//! A code and a field name are not predicates. They are a SCHEMA -- the names a
//! fleet survey groups by, which this module's own docs call "a breaking change
//! to the NDJSON row" -- and a schema is not derivable from the thing that emits
//! it. Writing it down twice is how a golden works: the test disagrees when the
//! writer moves, which is the entire point.
//!
//! The distinction matters because the tests that existed before these did the
//! derivable thing to the non-derivable one. They built their expectation from
//! `code()` and compared it against a row the writer had built from `code()`, so
//! both sides moved together and neither pinned anything.
//!
//! # What that left open, measured
//!
//! A mutation sweep of the parent module returned **12 survivors of 26** --
//! `NotCompared::code` could be replaced wholesale with `""` or `"xyzzy"`, and
//! every arm of `published_anomaly` and two of `anomaly_code` could be deleted,
//! all with a green suite. Separately, rewriting `published`'s count helper to
//! `*value * 7 + 1` -- every count in every entry wrong -- left 218 tests
//! passing.
//!
//! That is the field-labelling defect `row.rs` exists to make unrepresentable,
//! reappearing one level down: `row.rs` pairs a name with its value so position
//! cannot mislabel them, and then these functions hand-pair names with values
//! inside each entry, where nothing was watching.
//!
//! # Completeness
//!
//! Where the enum belongs to this crate, the expectation is written as an
//! exhaustive `match`, so a variant added without a golden does not compile.
//! `AnomalyKind` and `Source` are `#[non_exhaustive]` upstream and cannot be
//! matched exhaustively; for those the goldens are explicit instances, and the
//! `unclassified` fallback is asserted directly so that the arm which is
//! SUPPOSED to catch an unknown kind is distinguished from an arm that fell
//! through by accident.

use super::{
    Disagreement, NotCompared, ParseIncomplete, UNDESCRIBED, anomaly_code, described,
    published_anomaly,
};
use crate::row::Row;
use windows_topology_sys::{AnomalyKind, EnumerationAnomaly, Source};

/// The published value, rendered through the row's own writer.
///
/// Wrapped in a row rather than rendered directly, so the bytes under test are
/// the bytes a survey reads -- escaping, separators and all -- rather than a
/// second rendering written for the test.
fn rendered(value: crate::row::Value) -> String {
    let row = Row::new("x-test").with("entry", value).render();
    let opened = row.find("\"entry\":").expect("the entry is present") + "\"entry\":".len();
    row[opened..row.len() - 1].to_owned()
}

fn anomaly(source: Source, offset: usize, kind: AnomalyKind) -> EnumerationAnomaly {
    EnumerationAnomaly {
        source,
        offset,
        kind,
    }
}

#[test]
fn every_not_compared_code_is_the_one_the_row_promises() {
    // Exhaustive, so a seventh variant does not compile until it has a code
    // here. All six were unpinned: the sweep replaced the whole function with
    // `""` and with `"xyzzy"` and nothing noticed.
    let golden = |reason: &NotCompared| match reason {
        NotCompared::MachineChanged => "machine_changed",
        NotCompared::BracketNotEstablished => "bracket_not_established",
        NotCompared::CountsIncludeUnparsedRelations => "counts_include_unparsed_relations",
        NotCompared::ActiveProcessorCountFailed => "active_processor_count_failed",
        NotCompared::ActiveProcessorGroupCountFailed => "active_processor_group_count_failed",
        NotCompared::HighestNumaNodeFailed => "highest_numa_node_failed",
    };

    for reason in [
        NotCompared::MachineChanged,
        NotCompared::BracketNotEstablished,
        NotCompared::CountsIncludeUnparsedRelations,
        NotCompared::ActiveProcessorCountFailed,
        NotCompared::ActiveProcessorGroupCountFailed,
        NotCompared::HighestNumaNodeFailed,
    ] {
        assert_eq!(reason.code(), golden(&reason), "{reason:?}");
        assert_eq!(
            rendered(reason.published()),
            format!("{{\"code\":\"{}\"}}", golden(&reason)),
            "{reason:?}: a bare condition publishes its code and nothing else"
        );
    }
}

#[test]
fn every_disagreement_publishes_the_pair_it_carries() {
    // `parsed` and `counter` are the two numbers a survey compares, so swapping
    // the labels is the mislabelling defect in its purest form: the row still
    // parses and says the opposite of the truth.
    assert_eq!(
        rendered(
            Disagreement::OnlineProcessors {
                parsed: 12,
                counter: 16,
            }
            .published()
        ),
        r#"{"code":"online_processors","parsed":12,"counter":16}"#
    );
    assert_eq!(
        rendered(
            Disagreement::ProcessorGroups {
                parsed: 1,
                counter: 2,
            }
            .published()
        ),
        r#"{"code":"processor_groups","parsed":1,"counter":2}"#
    );
    assert_eq!(
        rendered(
            Disagreement::HighestNumaNode {
                parsed: Some(2),
                counter: 3,
            }
            .published()
        ),
        r#"{"code":"highest_numa_node","parsed":2,"counter":3}"#
    );
    // The absent parse renders as `null`, not as a number and not as an omitted
    // field: a survey must be able to tell "the parse saw no NUMA node" from
    // "the parse saw node 0".
    assert_eq!(
        rendered(
            Disagreement::HighestNumaNode {
                parsed: None,
                counter: 3,
            }
            .published()
        ),
        r#"{"code":"highest_numa_node","parsed":null,"counter":3}"#
    );
}

#[test]
fn every_parse_incomplete_shape_publishes_the_fields_its_variant_carries() {
    // One instance of each PAYLOAD SHAPE rather than of each variant: the twelve
    // counted variants share a single helper, and it was rewriting every one of
    // them wrongly that left 218 tests green.
    let cases = [
        (
            ParseIncomplete::ContradictoryCores { count: 3 },
            r#"{"code":"contradictory_cores","count":3}"#,
        ),
        (ParseIncomplete::NoPackages, r#"{"code":"no_packages"}"#),
        (
            ParseIncomplete::CacheLevelsWithoutPartitions { levels: vec![1, 2] },
            r#"{"code":"cache_levels_without_partitions","levels":[1,2]}"#,
        ),
        (
            ParseIncomplete::MeasuredButCountsAbsent {
                absent: vec!["packages"],
            },
            r#"{"code":"measured_but_counts_absent","absent":["packages"]}"#,
        ),
        (
            ParseIncomplete::PartitioningSummaryMissing { level: 3 },
            r#"{"code":"partitioning_summary_missing","level":3}"#,
        ),
        (
            ParseIncomplete::RelationsWithoutProcessors {
                cores: 4,
                packages: 7,
            },
            r#"{"code":"relations_without_processors","cores":4,"packages":7}"#,
        ),
        (
            ParseIncomplete::EnumerationsDisagreed {
                attempts: 2,
                walk_only: 5,
                cpu_sets_only: 9,
            },
            r#"{"code":"enumerations_disagreed","attempts":2,"walk_only":5,"cpu_sets_only":9}"#,
        ),
    ];

    for (entry, golden) in cases {
        assert_eq!(rendered(entry.published()), golden, "{entry:?}");
    }
}

#[test]
fn distinct_values_in_one_entry_are_not_interchangeable() {
    // **The labelling check, stated as a property rather than as another
    // golden.** The goldens above would still pass if two fields were swapped
    // AND both goldens were updated to match -- which is exactly what an author
    // mid-refactor does. This asks the narrower question a golden cannot: with
    // every value distinct, does each name carry ITS value?
    let relations = rendered(
        ParseIncomplete::RelationsWithoutProcessors {
            cores: 4,
            packages: 7,
        }
        .published(),
    );
    assert!(
        relations.contains(r#""cores":4"#) && relations.contains(r#""packages":7"#),
        "cores and packages must not be interchanged: {relations}"
    );

    let undersized = rendered(published_anomaly(&anomaly(
        Source::RelationshipWalk,
        64,
        AnomalyKind::Undersized {
            declared: 8,
            minimum: 48,
        },
    )));
    assert!(
        undersized.contains("\"declared\":8") && undersized.contains("\"minimum\":48"),
        "declared and minimum must not be interchanged: {undersized}"
    );
}

#[test]
fn every_named_anomaly_kind_has_a_code_of_its_own() {
    // A deleted arm here does not fail loudly -- it falls through to
    // `unclassified`, whose documented meaning is "this probe's vocabulary is
    // older than the crate". A real overrun would then be filed as an unknown
    // kind and mis-attributed across a fleet. Two of these arms were deletable
    // with a green suite.
    let cases = [
        (
            AnomalyKind::Undersized {
                declared: 1,
                minimum: 2,
            },
            "undersized",
        ),
        (
            AnomalyKind::OverrunsBuffer {
                declared: 3,
                remaining: 4,
            },
            "overruns_buffer",
        ),
        (
            AnomalyKind::TrailingBytes { remaining: 5 },
            "trailing_bytes",
        ),
        (
            AnomalyKind::TruncatedArray {
                declared: 6,
                decoded: 7,
            },
            "truncated_array",
        ),
    ];

    for (kind, golden) in cases {
        let described = format!("{kind:?}");
        let found = anomaly_code(&anomaly(Source::RelationshipWalk, 0, kind));
        assert_eq!(found, golden, "{described}");
        assert_ne!(
            found, "unclassified",
            "{described}: a kind this crate names must not fall through to the \
             catch-all, which says the vocabulary is older than the crate"
        );
    }
}

#[test]
fn every_anomaly_publishes_where_it_was_found_as_well_as_what() {
    // `source` and `offset` are the fields the module's docs give the reason
    // for -- the same kind at the same offset across a fleet is a different
    // finding from the same kind scattered. Every arm below was deletable.
    let cases = [
        (
            anomaly(
                Source::RelationshipWalk,
                64,
                AnomalyKind::Undersized {
                    declared: 8,
                    minimum: 48,
                },
            ),
            r#"{"code":"undersized","source":"relationship_walk","offset":64,"declared":8,"minimum":48}"#,
        ),
        (
            anomaly(
                Source::CpuSets,
                128,
                AnomalyKind::OverrunsBuffer {
                    declared: 96,
                    remaining: 32,
                },
            ),
            r#"{"code":"overruns_buffer","source":"cpu_sets","offset":128,"declared":96,"remaining":32}"#,
        ),
        (
            anomaly(
                Source::RelationshipWalk,
                256,
                AnomalyKind::TrailingBytes { remaining: 12 },
            ),
            r#"{"code":"trailing_bytes","source":"relationship_walk","offset":256,"remaining":12}"#,
        ),
        (
            anomaly(
                Source::CpuSets,
                512,
                AnomalyKind::TruncatedArray {
                    declared: 10,
                    decoded: 6,
                },
            ),
            r#"{"code":"truncated_array","source":"cpu_sets","offset":512,"declared":10,"decoded":6}"#,
        ),
    ];

    for (found, golden) in cases {
        assert_eq!(rendered(published_anomaly(&found)), golden, "{found:?}");
    }
}

#[test]
fn every_diagnostic_describes_itself() {
    // **Presence is machine-checked here; WORDING is not, and that is the
    // whole point of the seam.** This asserts only that each entry renders as
    // something a reader can act on -- never what it says -- so the prose stays
    // a review obligation while a blank stops being possible to ship.
    //
    // Two `Display` impls could be blanked with a green suite, and a reader
    // would have got `     - ` with nothing after the dash: indistinguishable
    // from a rendering bug, from a finding with nothing to say, and from a
    // stray newline. This test names that case and nothing else.
    let disagreements = [
        Disagreement::OnlineProcessors {
            parsed: 12,
            counter: 16,
        },
        Disagreement::ProcessorGroups {
            parsed: 1,
            counter: 2,
        },
        Disagreement::HighestNumaNode {
            parsed: Some(2),
            counter: 3,
        },
        Disagreement::HighestNumaNode {
            parsed: None,
            counter: 3,
        },
    ];
    for entry in &disagreements {
        let text = described(entry);
        assert_ne!(text, UNDESCRIBED, "{entry:?} renders blank");
        assert!(!text.trim().is_empty(), "{entry:?} renders blank");
    }

    // Exhaustive, so a seventh `NotCompared` must describe itself to compile.
    for entry in [
        NotCompared::MachineChanged,
        NotCompared::BracketNotEstablished,
        NotCompared::CountsIncludeUnparsedRelations,
        NotCompared::ActiveProcessorCountFailed,
        NotCompared::ActiveProcessorGroupCountFailed,
        NotCompared::HighestNumaNodeFailed,
    ] {
        let text = described(&entry);
        assert_ne!(text, UNDESCRIBED, "{entry:?} renders blank");
        assert!(!text.trim().is_empty(), "{entry:?} renders blank");
    }

    for entry in [
        ParseIncomplete::ContradictoryCores { count: 3 },
        ParseIncomplete::NoPackages,
        ParseIncomplete::CacheLevelsWithoutPartitions { levels: vec![1, 2] },
        ParseIncomplete::MeasuredButCountsAbsent {
            absent: vec!["packages"],
        },
        ParseIncomplete::PartitioningSummaryMissing { level: 3 },
        ParseIncomplete::RelationsWithoutProcessors {
            cores: 4,
            packages: 7,
        },
        ParseIncomplete::EnumerationsDisagreed {
            attempts: 2,
            walk_only: 5,
            cpu_sets_only: 9,
        },
    ] {
        let text = described(&entry);
        assert_ne!(text, UNDESCRIBED, "{entry:?} renders blank");
        assert!(!text.trim().is_empty(), "{entry:?} renders blank");
    }
}

#[test]
fn an_entry_that_says_nothing_is_called_out_rather_than_left_blank() {
    // The other half, and without it the test above cannot distinguish a
    // working `described` from one that returns its input unchanged.
    struct Silent;
    impl std::fmt::Display for Silent {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            Ok(())
        }
    }

    struct Blank;
    impl std::fmt::Display for Blank {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // Whitespace, not emptiness: a reader cannot tell the two apart on
            // the page, so neither may the check.
            f.write_str("   ")
        }
    }

    assert_eq!(described(&Silent), UNDESCRIBED);
    assert_eq!(described(&Blank), UNDESCRIBED);
    assert_eq!(described(&"a real description"), "a real description");
}
