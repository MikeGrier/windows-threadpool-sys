// Copyright (c) Mike Grier.
//! Tests for the report oracle.
//!
//! Half of these assert the oracle **accepts** something. That is deliberate,
//! and follows `windows-file-watcher`'s `ContractChecker`: an oracle that
//! rejects legal reports is as broken as one that passes illegal ones, and it
//! fails in the more expensive direction, because the noise trains a reader to
//! ignore it.
//!
//! The reports here are hand-written text rather than rendered from an
//! `Observation`. That is the point: the oracle reads artifacts, so it must be
//! testable with artifacts, including ones no renderer would currently produce.

use super::{Correspondence, check};

/// A report body with the shape the topology probe emits, for a host that is
/// unremarkable and agrees with itself.
fn clean_report() -> String {
    [
        "host:  x86_64 16p/8c",
        "== what does this machine look like? ==",
        "",
        "processors (online) : 16",
        "processor groups    : 1",
        "packages            : 1",
        "NUMA domains        : 1 (0 with no processors)",
        "physical cores      : 8",
        "  cores with SMT    : 8",
        "  efficiency classes: [0]",
        "",
        "caches:",
        "  L1    8 domain(s), processors per domain: [2, 2, 2, 2, 2, 2, 2, 2]",
        "  L3    1 domain(s), processors per domain: [16]",
        "",
        "outermost cache that partitions the processors it covers: L1 (8 domains)",
        "",
        "domains each policy would produce:",
        "  single                             1",
        "  by-core                            8",
        "",
        "cross-check against independently read Win32 counters:",
        "  GetActiveProcessorCount     : 16",
        "  GetActiveProcessorGroupCount: 1",
        "  GetNumaHighestNodeNumber    : 0",
        "  => agree. Every check this probe could make was made and matched.",
        r#"{"reason":"x-probe-topology","arch":"x86_64","processors":16,"groups":1,"packages":1,"numa_domains":1,"numa_domains_without_processors":0,"cores":8,"efficiency_classes":[0],"caches":[{"level":1,"domains":8},{"level":3,"domains":1}],"outermost_partitioning_cache_level":1,"policies":{"single":1,"by-core":8},"cross_check":"agree","parse_incomplete":0}"#,
    ]
    .join("\n")
}

// --- must reject ------------------------------------------------------------

#[test]
fn an_alarm_beside_an_agreeing_prose_verdict_is_a_violation() {
    // The original defect, reduced: `BUG IN THIS PROBE` and `=> agree` in one
    // report. Both statements were locally true and they cannot both describe
    // the same run.
    let report = clean_report().replace(
        "cross-check against independently read Win32 counters:",
        "BUG IN THIS PROBE: the topology crate named L3 as the outermost\ncross-check against independently read Win32 counters:",
    );

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::AlarmWithAgreeingVerdict {
                verdict_source: "prose",
                ..
            }
        )),
        "an alarm printed beside `=> agree` must be reported, got {violations:#?}"
    );
}

#[test]
fn an_alarm_beside_an_agreeing_ndjson_verdict_is_a_violation() {
    // The same contradiction reaching a mining pass instead of a reader. It is
    // reported separately because the two consumers are separate: a fleet
    // survey never sees the prose.
    let report = clean_report()
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => INCOMPLETE. Nothing this probe compared disagreed.",
        )
        .replace(
            "cross-check against independently read Win32 counters:",
            "BUG IN THIS PROBE: something\ncross-check against independently read Win32 counters:",
        );
    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::AlarmWithAgreeingVerdict {
                verdict_source: "ndjson",
                ..
            }
        )),
        "an alarm beside `\"cross_check\":\"agree\"` must be reported, got {violations:#?}"
    );
}

#[test]
fn a_count_the_prose_and_the_ndjson_disagree_about_is_a_violation() {
    let report = clean_report().replace("processors (online) : 16", "processors (online) : 8");

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "online processors",
                ..
            }
        )),
        "two renderings of the processor count must agree, got {violations:#?}"
    );
}

#[test]
fn a_class_count_rendered_where_the_class_list_belongs_is_a_violation() {
    // The historical defect this pair exists for: the NDJSON emitted the class
    // COUNT under a plural name, so a single-class host printed
    // `"efficiency_classes":1` beside a prose `efficiency classes: [0]`. Same
    // fact, same report, and the two readings differ -- one says "one class",
    // the other says "class one".
    let report = clean_report().replace(r#""efficiency_classes":[0]"#, r#""efficiency_classes":1"#);

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "efficiency classes",
                ..
            }
        )),
        "a class count where the class list belongs must be reported, got {violations:#?}"
    );
}

#[test]
fn a_verdict_the_prose_and_the_ndjson_disagree_about_is_a_violation() {
    let report = clean_report().replace(r#""cross_check":"agree""#, r#""cross_check":"disagree""#);

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "cross-check verdict",
                ..
            }
        )),
        "the verdict must read the same in both renderings, got {violations:#?}"
    );
}

#[test]
fn a_bare_hardware_claim_under_an_incomplete_parse_is_a_violation() {
    let report = clean_report()
        .replace(
            "  efficiency classes: [0]",
            "  efficiency classes: [0, 1]\n  (heterogeneous: an I/O thread left unconstrained can land on an",
        )
        .replace(r#""efficiency_classes":[0]"#, r#""efficiency_classes":[0,1]"#)
        .replace(r#""parse_incomplete":0"#, r#""parse_incomplete":2"#);

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::UncaveatedClaimUnderDoubt {
                claim: "heterogeneity",
                ..
            }
        )),
        "a hardware claim stated bare under a short parse must be reported, got {violations:#?}"
    );
}

#[test]
fn a_bare_hardware_claim_under_a_disagreeing_cross_check_is_a_violation() {
    // The other half of `parse_in_doubt`. A disagreement is doubt about the
    // parse just as much as an incomplete one, and the renderer's gate covers
    // both -- so an oracle that only knew about `parse_incomplete` would pass
    // exactly half the cases the rule is written for.
    let report = clean_report()
        .replace(
            "  efficiency classes: [0]",
            "  efficiency classes: [0, 1]\n  (heterogeneous: an I/O thread left unconstrained can land on an",
        )
        .replace(r#""efficiency_classes":[0]"#, r#""efficiency_classes":[0,1]"#)
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => DISAGREE. This is a finding, not a nuisance:",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"disagree""#);

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::UncaveatedClaimUnderDoubt {
                claim: "heterogeneity",
                ..
            }
        )),
        "a hardware claim stated bare under a disagreement must be reported, got {violations:#?}"
    );
}

#[test]
fn every_double_rendered_fact_is_actually_read() {
    // The failure mode that would make this whole oracle worthless, and would
    // look exactly like success: a prose label that does not match what the
    // renderer emits makes the lookup return `None`, the comparison is skipped,
    // and the report passes having been checked for nothing.
    //
    // So each pair is exercised individually rather than trusted. Corrupting
    // one prose value must produce one violation naming that fact; if a label
    // ever drifts from the renderer, the corresponding case here stops firing
    // and this test fails rather than the oracle going quietly blind.
    //
    // The labels themselves were confirmed against a real `probe-topology` run,
    // which is what makes the fixture above a fixture and not a guess.
    for (label, wrong) in [
        ("processors (online) : 16", "processors (online) : 99"),
        ("processor groups    : 1", "processor groups    : 99"),
        ("packages            : 1", "packages            : 99"),
        ("physical cores      : 8", "physical cores      : 99"),
        ("  efficiency classes: [0]", "  efficiency classes: [9]"),
    ] {
        let report = clean_report().replace(label, wrong);
        let violations = check(&report);

        assert!(
            violations
                .iter()
                .any(|v| matches!(v, Correspondence::ProseAndNdjsonDisagree { .. })),
            "corrupting `{label}` produced no violation, so the oracle is not \
             reading that line at all -- got {violations:#?}"
        );
    }
}

// --- the cells the M2.4 matrix walk added ------------------------------------

#[test]
fn a_counter_that_contradicts_the_enumeration_under_an_agreeing_verdict_is_a_violation() {
    // The rule closest to what this probe is *for*, and the original defect in
    // its purest form: the report printing its own contradicting evidence
    // directly above a verdict denying it. The whole run exists to compare an
    // independently read counter against the enumeration, so a mismatch is the
    // finding -- and `agree` says there was none.
    let report = clean_report().replace(
        "  GetActiveProcessorCount     : 16",
        "  GetActiveProcessorCount     : 8",
    );

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "active processor count against the enumeration",
                ..
            }
        )),
        "a counter disagreeing with the enumeration under `agree` must be \
         reported, got {violations:#?}"
    );
}

#[test]
fn a_contradicting_counter_is_accepted_when_the_verdict_reports_it() {
    // The legal shape, and the one the probe exists to produce. Rejecting it
    // would fire on every host that actually has the disagreement this probe
    // hunts for -- the run most worth reading.
    let report = clean_report()
        .replace(
            "  GetActiveProcessorCount     : 16",
            "  GetActiveProcessorCount     : 8",
        )
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => DISAGREE. This is a finding, not a nuisance:",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"disagree""#);

    assert_eq!(check(&report), Vec::new());
}

#[test]
fn the_highest_numa_node_number_is_not_compared_against_the_domain_count() {
    // Deliberately absent from the counter rule, and pinned so it stays absent.
    // `GetNumaHighestNodeNumber` reports the largest node NUMBER, which the
    // report itself says is not a count; comparing it against `numa_domains`
    // would manufacture a disagreement on any machine with sparse node
    // numbering. Over-constraining is the same defect as under-specifying.
    let report = clean_report().replace(
        "  GetNumaHighestNodeNumber    : 0",
        "  GetNumaHighestNodeNumber    : 7",
    );

    assert_eq!(check(&report), Vec::new());
}

#[test]
fn a_policy_count_the_two_renderings_disagree_about_is_a_violation() {
    // The domain count per policy is the answer the whole report exists to
    // give, so two renderings of it disagreeing misleads exactly the reader who
    // came for it.
    let report = clean_report().replace(r#""by-core":8"#, r#""by-core":4"#);

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "policy domain count",
                ..
            }
        )),
        "the policy table and the policies object must agree, got {violations:#?}"
    );
}

#[test]
fn a_cache_domain_count_the_two_renderings_disagree_about_is_a_violation() {
    let report = clean_report().replace(r#"{"level":3,"domains":1}"#, r#"{"level":3,"domains":9}"#);

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "cache domain count",
                ..
            }
        )),
        "the cache table and the caches array must agree, got {violations:#?}"
    );
}

#[test]
fn an_outermost_level_the_two_renderings_disagree_about_is_a_violation() {
    let report = clean_report().replace(
        r#""outermost_partitioning_cache_level":1"#,
        r#""outermost_partitioning_cache_level":3"#,
    );

    let violations = check(&report);

    assert!(
        violations.iter().any(|v| matches!(
            v,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "outermost partitioning cache level",
                ..
            }
        )),
        "the named outermost level must match the machine-readable one, got {violations:#?}"
    );
}

#[test]
fn a_nested_container_is_read_whole_rather_than_to_its_first_closer() {
    // `caches` is an array OF objects, so a reader stopping at the first `}`
    // would see only its first entry -- and would then silently skip every
    // later level rather than compare it. This corrupts the LAST cache entry,
    // which only a balanced read can reach.
    let report = clean_report().replace(r#"{"level":3,"domains":1}"#, r#"{"level":3,"domains":5}"#);

    assert!(
        !check(&report).is_empty(),
        "a disagreement in the last element of a nested container must still be \
         found, or the container is being truncated at its first closer"
    );
}

// --- must accept ------------------------------------------------------------

#[test]
fn a_report_that_agrees_with_itself_is_accepted() {
    assert_eq!(check(&clean_report()), Vec::new());
}

#[test]
fn an_alarm_with_a_verdict_that_is_not_agree_is_accepted() {
    // The legal shape of an alarm, and the one the fix produced. Rejecting it
    // would make the oracle unusable on exactly the reports it was written to
    // protect.
    let report = clean_report()
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => INCOMPLETE. Nothing this probe compared disagreed, but this run",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"incomplete""#)
        .replace(
            "cross-check against independently read Win32 counters:",
            "BUG IN THIS PROBE: something\ncross-check against independently read Win32 counters:",
        );
    assert_eq!(check(&report), Vec::new());
}

#[test]
fn a_hardware_claim_with_its_caveat_under_doubt_is_accepted() {
    // Doubt plus a claim is legal when the claim is caveated. This is the
    // shape the renderer actually produces, so an oracle that rejected it
    // would fire on every heterogeneous host with a short parse.
    let report = clean_report()
        .replace(
            "  efficiency classes: [0]",
            "  efficiency classes: [0, 1]\n  (heterogeneous: an I/O thread left unconstrained can land on an\n   (This run did not establish that the parse is whole, and the classes",
        )
        .replace(r#""efficiency_classes":[0]"#, r#""efficiency_classes":[0,1]"#)
        .replace(r#""parse_incomplete":0"#, r#""parse_incomplete":2"#);

    assert_eq!(check(&report), Vec::new());
}

#[test]
fn a_bare_hardware_claim_with_no_doubt_reported_is_accepted() {
    // The common case on a healthy heterogeneous host: the claim is bare
    // because there is nothing to caveat.
    let report = clean_report()
        .replace(
            "  efficiency classes: [0]",
            "  efficiency classes: [0, 1]\n  (heterogeneous: an I/O thread left unconstrained can land on an",
        )
        .replace(r#""efficiency_classes":[0]"#, r#""efficiency_classes":[0,1]"#);

    assert_eq!(check(&report), Vec::new());
}

#[test]
fn a_report_with_no_ndjson_line_is_accepted() {
    // `report_unmeasured` and any future prose-only report. Every NDJSON read
    // is optional by construction, so a missing line is silence rather than a
    // violation.
    let prose = clean_report()
        .lines()
        .filter(|line| !line.starts_with('{'))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(check(&prose), Vec::new());
}

#[test]
fn a_shorter_ndjson_object_is_accepted() {
    // `report_unmeasured` emits only `reason`, `arch` and `cross_check`. Fields
    // the oracle knows about but the report does not carry are absent, not
    // wrong.
    let report = [
        "host:  x86_64 16p/8c",
        "the topology could not be read: something went wrong",
        r#"{"reason":"x-probe-topology","arch":"x86_64","cross_check":"not_measured"}"#,
    ]
    .join("\n");

    assert_eq!(check(&report), Vec::new());
}

#[test]
fn the_two_class_list_punctuations_are_read_as_the_same_list() {
    // The prose renders `[0, 1]` through `Debug` and the NDJSON `[0,1]` through
    // a join. They are the same fact, and an oracle that compared them as text
    // would report every multi-class host as a contradiction.
    let report = clean_report()
        .replace("  efficiency classes: [0]", "  efficiency classes: [0, 1]")
        .replace(
            r#""efficiency_classes":[0]"#,
            r#""efficiency_classes":[0,1]"#,
        );

    assert_eq!(check(&report), Vec::new());
}

#[test]
fn a_banner_naming_a_different_machine_from_the_body_is_a_violation() {
    // The M2.5 defect. Both halves are locally correct -- the banner faithfully
    // renders one topology and the body another -- and the report reconciles
    // them nowhere, so a reader deciding whether two runs are comparable is
    // reading a line about a machine the numbers did not come from.
    let report = clean_report().replace("host:  x86_64 16p/8c", "host:  x86_64 8p/8c");
    assert!(
        !report.contains("16p/8c"),
        "the sabotage must actually have landed, or this test proves nothing"
    );

    assert_eq!(
        check(&report),
        vec![Correspondence::BannerDisagreesWithBody {
            banner: "8".to_owned(),
            body: "16".to_owned(),
        }]
    );
}

#[test]
fn a_banner_for_a_host_that_could_not_be_read_is_accepted() {
    // `Fingerprint::discover` failing renders `UNKNOWN` with no count in it.
    // There is nothing to relate, and reporting a contradiction would turn a
    // gap in the measurement into a claim about the report -- the inversion
    // this whole crate is built to avoid.
    let report = clean_report().replace(
        "host:  x86_64 16p/8c",
        "host:  UNKNOWN -- topology discovery failed: access denied",
    );

    assert_eq!(check(&report), Vec::new());
}

#[test]
fn a_tainted_banner_is_still_read_for_its_count() {
    // An unmeasured topology renders behind a `!!...!! ` prefix. The taint says
    // the numbers are not to be trusted as hardware -- it does not excuse the
    // banner from naming the same numbers the body does, and a reader
    // reconciling the two is exactly who the marker is for.
    let report = clean_report().replace("host:  x86_64 16p/8c", "host:  !!assumed!! x86_64 8p/8c");

    assert_eq!(
        check(&report),
        vec![Correspondence::BannerDisagreesWithBody {
            banner: "8".to_owned(),
            body: "16".to_owned(),
        }]
    );
}

#[test]
fn the_second_banner_line_of_a_disagreeing_bracket_is_not_compared() {
    // When the endpoint readings differ, `attribution` prints the other reading
    // too and says plainly that which one names the machine was not
    // established. The body deliberately does not describe that second reading,
    // so comparing it here would report a contradiction as a defect when it is
    // the renderer being honest -- an oracle that over-constrains fails in the
    // more expensive direction.
    let report = clean_report().replace(
        "host:  x86_64 16p/8c",
        "host:  x86_64 16p/8c\nhost:  x86_64 8p/8c\nHOST READINGS DISAGREE: the two readings above \
         bracket the measurement\nand differ, so which of them names the machine the body below \
         describes\nwas not established.",
    );

    assert_eq!(check(&report), Vec::new());
}
