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

/// A fingerprint-shaped banner naming the architecture this build actually
/// targets.
///
/// **Not a literal, because the renderer publishes `std::env::consts::ARCH`.**
/// A banner hard-coding `x86_64` agrees with the body on an x86_64 host and
/// contradicts it everywhere else, so every test that feeds a banner to the
/// RENDERER failed on `i686-pc-windows-msvc` -- five of them, each reporting
/// `prose: "x86_64"` against `ndjson: "x86"`. CI builds `aarch64` but does not
/// run the suite there, so the fleet never saw it either.
///
/// Fixtures built entirely by hand are unaffected: their banner and their NDJSON
/// both say `x86_64`, so they agree with each other whatever the host is. Only
/// the ones that mix a literal banner with a rendered body were wrong.
fn host_banner(rest: &str) -> String {
    format!("host:  {} {rest}", std::env::consts::ARCH)
}

/// A fingerprint this build can always produce, naming the build's architecture.
///
/// Tests that need a successful bracket reading construct one rather than
/// calling `Fingerprint::discover()`, so they do not depend on the host being
/// able to read its own topology -- which is a failure this crate exists to
/// report, not one a test should be defeated by.
fn built_fingerprint() -> windows_placement_probe::fingerprint::Fingerprint {
    windows_placement_probe::fingerprint::Fingerprint {
        arch: std::env::consts::ARCH,
        processors: 16,
        cores: 8,
        smt: true,
        partitioning_cache_level: Some(2),
        cache_domain_sizes: vec![8, 8],
        efficiency_classes: vec![(0, 16)],
        numa_node_sizes: vec![16],
        provenance: windows_topology_sys::Provenance::Measured,
    }
}

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
        r#"{"reason":"x-probe-topology","arch":"x86_64","processors":16,"groups":1,"packages":1,"numa_domains":1,"numa_domains_without_processors":0,"cores":8,"efficiency_classes":[0],"caches":[{"level":1,"domains":8},{"level":3,"domains":1}],"outermost_partitioning_cache_level":1,"outermost_partitioning_cache":"level","policies":{"single":1,"by-core":8},"cross_check":"agree","parse_incomplete":0}"#,
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
    //
    // **The verdict moves with the doubt, and an earlier version of this
    // fixture forgot that.** It set `"parse_incomplete":2` while leaving the
    // verdict at `agree`, which the renderer cannot emit: it publishes the rule
    // that a non-empty `parse_incomplete` forces the verdict away from `agree`.
    // So the comment above claimed "the shape the renderer actually produces"
    // about a shape it cannot produce -- caught when the rule that reads those
    // counts was added and rejected this fixture. Both renderings of the verdict
    // move together here, because the report renders it twice.
    let report = clean_report()
        .replace(
            "  efficiency classes: [0]",
            "  efficiency classes: [0, 1]\n  (heterogeneous: an I/O thread left unconstrained can land on an\n   (This run did not establish that the parse is whole, and the classes",
        )
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => INCOMPLETE. Nothing this probe compared disagreed, but this run\n\
             \x20    did not establish that the parse is consistent:\n\
             \x20    - a cache record failed to decode\n\
             \x20    - a second cache record failed to decode",
        )
        .replace(r#""efficiency_classes":[0]"#, r#""efficiency_classes":[0,1]"#)
        .replace(r#""cross_check":"agree""#, r#""cross_check":"incomplete""#)
        .replace(
            r#""parse_incomplete":0}"#,
            r#""parse_incomplete":2,"not_compared":0,"enumeration_anomalies":0}"#,
        );

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

#[test]
fn an_indeterminate_attribution_is_not_a_banner_violation() {
    // **The report says the correspondence was not established, so the oracle
    // must not assert it.** `attribution` on this branch takes the two bracket
    // readings and renders, when they differ:
    //
    //   HOST READINGS DISAGREE: ... which of them names the machine the body
    //   below describes was not established.
    //
    // The body comes from a `measure()` between them, so the FIRST banner line
    // is one endpoint and need not describe the body. Reporting a contradiction
    // there would be the oracle over-claiming exactly as the renderer went to
    // trouble not to -- and over-constraining is the failure this file's other
    // half exists to catch.
    //
    // This is the seam the origin branch closed with `measure_observed`, which
    // builds the banner from the body's own topology. That construction change
    // is not in this peel, so the ambiguity is real here and the oracle has to
    // respect it.
    let report = clean_report().replace(
        "host:  x86_64 16p/8c",
        "host:  x86_64 8p/4c\n\
         host:  x86_64 16p/8c\n\
         HOST READINGS DISAGREE: the two readings above bracket the measurement\n\
         and differ, so which of them names the machine the body below describes\n\
         was not established.",
    );

    assert_eq!(
        check(&report),
        Vec::new(),
        "the first banner reading names 8 processors and the body 16, which the \
         report itself declines to call a contradiction"
    );
}

#[test]
fn an_unestablished_host_is_not_a_banner_violation() {
    // The other indeterminate form: at least one bracket reading failed, so
    // nothing confirmed the host held still. Same reasoning, different text,
    // and it is a separate arm of `attribution`'s match -- checking only the
    // disagree case would leave this one asserting a correspondence the report
    // does not claim.
    //
    // **The FAILED reading is the second one, and that ordering is the test.**
    // An earlier version put `UNKNOWN` first, which is equally legal -- the
    // `_` arm of `attribution` covers (Err, Ok), (Ok, Err) and (Err, Err) --
    // but it made this test vacuous: `processors_in_banner("host:  UNKNOWN")`
    // finds no count, so the rule returned at the count guard and never
    // reached the disclaimer. Measured: deleting the `HOST NOT ESTABLISHED`
    // arm of the exemption left that version, and the whole suite, green.
    // Putting the successful reading first gives the rule a count to compare,
    // so the exemption is the only thing that can suppress the violation.
    let report = clean_report().replace(
        "host:  x86_64 16p/8c",
        "host:  x86_64 8p/4c\n\
         host:  UNKNOWN\n\
         HOST NOT ESTABLISHED: at least one of the two readings that bracket the measurement\n\
         failed, so nothing confirmed the machine held still under it.",
    );

    assert_eq!(
        check(&report),
        Vec::new(),
        "a failed bracket reading is not evidence the banner contradicts the body"
    );
}

#[test]
fn the_unestablished_host_fixture_would_be_a_violation_without_the_disclaimer() {
    // The control that keeps the test above honest. Same report, same 8-versus-16
    // contradiction, with only the disclaimer removed -- and now it MUST be a
    // violation. Without this, a future edit that stops the rule reaching the
    // comparison at all would leave the acceptance test passing for the wrong
    // reason, which is precisely how the previous version went vacuous.
    let report = clean_report().replace(
        "host:  x86_64 16p/8c",
        "host:  x86_64 8p/4c\n\
         host:  UNKNOWN",
    );

    assert_eq!(
        check(&report),
        vec![Correspondence::BannerDisagreesWithBody {
            banner: "8".to_owned(),
            body: "16".to_owned(),
        }],
        "with no disclaimer the banner's 8 processors contradict the body's 16, so \
         the acceptance test above is established by the exemption rather than by \
         the comparison being unreachable"
    );
}
#[test]
fn an_architecture_the_banner_and_the_ndjson_disagree_about_is_a_violation() {
    // Found by a review corrupting the NDJSON `arch` and watching the oracle
    // accept it. Both renderings come from `std::env::consts::ARCH` today, so
    // they cannot currently differ -- which is a fact about the renderer rather
    // than a contract, and exactly the kind of coincidence this oracle is built
    // not to lean on.
    let report = clean_report().replace(r#""arch":"x86_64""#, r#""arch":"aarch64""#);

    assert!(
        check(&report).iter().any(|violation| matches!(
            violation,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "architecture",
                ..
            }
        )),
        "the banner names x86_64 and the body aarch64: {:#?}",
        check(&report)
    );
}

#[test]
fn a_tainted_banner_is_still_read_for_its_architecture() {
    // The taint prefix is a rendering of doubt about the READING, not a
    // different machine, so the architecture behind it is still the one the
    // banner claims. Skipping it here would quietly drop the correspondence on
    // exactly the reports where a reader most wants it checked.
    let report = clean_report()
        .replace(
            "host:  x86_64 16p/8c",
            "host:  !!assumed!! !!taint!! x86_64 16p/8c",
        )
        .replace(r#""arch":"x86_64""#, r#""arch":"aarch64""#);

    assert!(
        check(&report).iter().any(|violation| matches!(
            violation,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "architecture",
                ..
            }
        )),
        "a tainted banner still names an architecture: {:#?}",
        check(&report)
    );
}

#[test]
fn an_architecture_disagreement_is_read_on_an_unmeasured_report_too() {
    // The architecture rule was written below the processor-count guard, which
    // confined it to MEASURED reports without saying so: `report_unmeasured`
    // renders `arch` and no `processors`, so the guard returned first and the
    // architecture went uncompared on exactly the reports that carry least
    // else. Found by a review; measured before the fix as `check()` returning
    // no violation at all for the report below.
    //
    // This is the shape `report_unmeasured` emits when the bracket reading
    // succeeded and the measurement did not, so the banner names a real
    // architecture while the body is the short object.
    let report = "host:  x86_64 16p/8c\n\
         MachineMemoryTopology::discover failed: a simulated failure\n\
         {\"reason\":\"x-probe-topology\",\"arch\":\"aarch64\",\"cross_check\":\"not_measured\"}\n";

    assert_eq!(
        check(report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "architecture",
            prose: "x86_64".to_owned(),
            ndjson: "aarch64".to_owned(),
        }],
        "an unmeasured report renders the architecture twice like any other, so \
         the absence of a processor count must not suppress the comparison"
    );
}

#[test]
fn a_cpu_set_only_numa_count_the_two_renderings_disagree_about_is_a_violation() {
    // The third double-rendered fact this module shipped without reading, after
    // `arch` and the outermost-cache discriminator. Found by a review, not by
    // the oracle's own coverage -- which is the argument for M2.10's approach of
    // deriving the fact set rather than extending it by hand.
    //
    // The prose line is rendered ONLY when the count is above zero, so the
    // fixture has to add it: `clean_report()` describes a host with none.
    let report = clean_report().replace(
        "NUMA domains        : 1 (0 with no processors)",
        "NUMA domains        : 1 (0 with no processors)\n  (5 reported only by CPU Sets, never by the relationship walk:",
    );
    let report = report.replace(
        r#""numa_domains_without_processors":0"#,
        r#""numa_domains_without_processors":0,"numa_domains_only_in_cpu_sets":9"#,
    );

    assert!(
        check(&report).iter().any(|violation| matches!(
            violation,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "NUMA domains reported only by CPU Sets",
                ..
            }
        )),
        "the prose says 5 and the body 9: {:#?}",
        check(&report)
    );
}

#[test]
fn a_report_with_no_cpu_set_only_line_is_accepted() {
    // The conditional half. On a host where the count is zero the renderer emits
    // no such line, and absence is silence rather than a disagreement with the
    // NDJSON's `0`. An oracle that read the missing line as a mismatch would
    // fire on almost every host -- the over-constraining failure this file's
    // acceptance half exists to catch.
    let report = clean_report().replace(
        r#""numa_domains_without_processors":0"#,
        r#""numa_domains_without_processors":0,"numa_domains_only_in_cpu_sets":0"#,
    );

    assert_eq!(
        check(&report),
        Vec::new(),
        "a host with no CPU-Set-only domains renders no line to compare"
    );
}

#[test]
fn a_policy_the_ndjson_renames_is_a_violation() {
    // Gap 4, found by a review: the policy NAME is a double-rendered fact, and
    // locating the NDJSON entry by it made a failed lookup silent. Before this
    // rule, renaming one side alone left the oracle with nothing to compare and
    // the report was accepted.
    let report = clean_report().replace(r#""by-core":8"#, r#""by-cores":8"#);

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "policy names",
            prose: "by-core, single".to_owned(),
            ndjson: "by-cores, single".to_owned(),
        }],
        "the prose names a policy the NDJSON does not"
    );
}

#[test]
fn a_policy_missing_from_the_ndjson_is_a_violation() {
    // The same silence in its other form. Dropping the entry leaves the prose
    // row with nothing to match, which the per-entry comparison cannot report.
    let report = clean_report().replace(r#","by-core":8"#, "");

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "policy names",
            prose: "by-core, single".to_owned(),
            ndjson: "single".to_owned(),
        }],
        "a policy the prose reports is absent from the machine-readable line"
    );
}

#[test]
fn a_policy_only_the_ndjson_reports_is_a_violation() {
    // The third form, and the one a per-entry loop over PROSE rows can never
    // see: an entry the prose never mentions is not iterated at all.
    let report = clean_report().replace(r#""by-core":8}"#, r#""by-core":8,"by-l3":2}"#);

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "policy names",
            prose: "by-core, single".to_owned(),
            ndjson: "by-core, by-l3, single".to_owned(),
        }],
        "the NDJSON carries a policy the prose does not report"
    );
}

#[test]
fn a_cache_the_ndjson_moves_to_another_level_is_a_violation() {
    // The cache half of gap 4, and the exact case the review named: the prose
    // still reads `L3` while the NDJSON calls it level 9, so the level lookup
    // matches nothing and the domain count goes uncompared.
    let report = clean_report().replace(r#"{"level":3,"domains":1}"#, r#"{"level":9,"domains":1}"#);

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "cache levels",
            prose: "1, 3".to_owned(),
            ndjson: "1, 9".to_owned(),
        }],
        "the two renderings disagree about which cache levels exist"
    );
}

#[test]
fn collections_rendered_in_different_orders_are_accepted() {
    // The acceptance half, and the reason the comparison sorts. Nothing obliges
    // the NDJSON to emit its entries in the prose's order, so a rule comparing
    // sequences would report a contradiction about ordering that neither
    // rendering claims. Both sides get the same comparator, so this stays
    // consistent whatever order either chooses.
    let report = clean_report()
        .replace(
            r#""policies":{"single":1,"by-core":8}"#,
            r#""policies":{"by-core":8,"single":1}"#,
        )
        .replace(
            r#""caches":[{"level":1,"domains":8},{"level":3,"domains":1}]"#,
            r#""caches":[{"level":3,"domains":1},{"level":1,"domains":8}]"#,
        );

    assert_eq!(
        check(&report),
        Vec::new(),
        "the same entries in a different order are the same entries"
    );
}

#[test]
fn an_agreeing_verdict_beside_a_nonzero_parse_incomplete_is_a_violation() {
    // Gap 5, and the reason it carries a correctness question rather than only
    // a completeness one. The renderer publishes the rule where it emits the
    // NDJSON: anomalies populate `parse_incomplete`, a non-empty
    // `parse_incomplete` forces the verdict away from `agree`, so
    // `cross_check == "agree"` implies no record failed to decode. An `agree`
    // beside a nonzero count is therefore the report contradicting its own
    // published rule, in the field a mining pass trusts before any other.
    let report = clean_report().replace(r#""parse_incomplete":0"#, r#""parse_incomplete":2"#);

    assert_eq!(
        check(&report),
        vec![Correspondence::AlarmWithAgreeingVerdict {
            alarm: r#""parse_incomplete":2"#.to_owned(),
            verdict_source: "ndjson",
        }],
        "a parse that did not complete cannot sit beside a verdict saying every \
         check matched"
    );
}

#[test]
fn an_agreeing_verdict_beside_a_nonzero_anomaly_count_is_a_violation() {
    // The same rule reached through the other field it names. Anomalies populate
    // `parse_incomplete`, so an `agree` verdict rules both out.
    let report = clean_report().replace(
        r#""parse_incomplete":0}"#,
        r#""parse_incomplete":0,"enumeration_anomalies":1}"#,
    );

    assert_eq!(
        check(&report),
        vec![Correspondence::AlarmWithAgreeingVerdict {
            alarm: r#""enumeration_anomalies":1"#.to_owned(),
            verdict_source: "ndjson",
        }],
        "a dropped enumeration record cannot sit beside an agreeing verdict"
    );
}

#[test]
fn a_not_compared_count_the_two_renderings_disagree_about_is_a_violation() {
    // The completeness half. Under DISAGREE the prose labels each skipped check
    // on its own line, so the two renderings of how many there were can be
    // compared directly.
    // `clean_report()` renders no `not_compared` at all -- it is narrower than
    // the real renderer -- so the field has to be added here rather than
    // replaced. Written as a replacement of an absent key, this test passed
    // while checking nothing, which is how the first draft of it went green.
    let report = clean_report()
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => DISAGREE. This is a finding, not a nuisance:\n\
             \x20    - the group count disagrees\n\
             \x20    (not compared) GetNumaHighestNodeNumber is not a count",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"disagree""#)
        .replace(
            r#""parse_incomplete":0}"#,
            r#""parse_incomplete":0,"not_compared":4,"enumeration_anomalies":0}"#,
        );

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "not compared count",
            prose: "1".to_owned(),
            ndjson: "4".to_owned(),
        }],
        "the prose lists one skipped check and the NDJSON claims four"
    );
}

#[test]
fn an_incomplete_verdict_listing_fewer_entries_than_it_counts_is_a_violation() {
    // Under INCOMPLETE both kinds render as a bare `- `, so only their TOTAL is
    // recoverable from the prose -- and that total is what this compares.
    // Claiming to separate them here would be reading a distinction the prose
    // does not draw.
    let report = clean_report()
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => INCOMPLETE. Nothing this probe compared disagreed, but this run\n\
             \x20    did not establish that the parse is consistent:\n\
             \x20    - a cache record failed to decode",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"incomplete""#)
        .replace(
            r#""parse_incomplete":0}"#,
            r#""parse_incomplete":3,"not_compared":0,"enumeration_anomalies":0}"#,
        );

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "incomplete-verdict listing count",
            prose: "1".to_owned(),
            ndjson: "3".to_owned(),
        }],
        "one entry is listed where the counts total three"
    );
}

#[test]
fn a_nonzero_not_compared_beside_an_incomplete_verdict_is_accepted() {
    // The acceptance half, and it pins the direction of the rule. The renderer
    // states the implication ONE WAY: a run whose counter failed to read has a
    // complete parse and still reports `incomplete`. So a nonzero
    // `not_compared` is legal here, and a rule asserting it forces the verdict
    // would be claiming more than the contract does.
    let report = clean_report()
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => INCOMPLETE. Nothing this probe compared disagreed, but this run\n\
             \x20    did not establish that the parse is consistent:\n\
             \x20    - GetActiveProcessorCount could not be read",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"incomplete""#)
        .replace(
            r#""parse_incomplete":0}"#,
            r#""parse_incomplete":0,"not_compared":1,"enumeration_anomalies":0}"#,
        );

    assert_eq!(
        check(&report),
        Vec::new(),
        "a counter that could not be read is exactly what an incomplete verdict \
         reports, and the parse is untouched by it"
    );
}

/// The prose each discriminator arm announces itself with, as the renderer
/// writes it. Used to exercise every arm rather than the one this host happens
/// to produce.
fn partitioning_prose(arm: &str) -> &'static str {
    match arm {
        "level" => "\noutermost cache that partitions the processors it covers: L1 (8 domains)",
        "none" => {
            "\nno cache level reported more than one domain, so nothing here divides\nthe work by cache."
        }
        "no_levels_reported" => {
            "\nno cache levels were reported at all, so nothing here says whether a\ncache boundary divides this machine."
        }
        "not_unique" => {
            "\nat least one cache level reported more than one distinct domain, but\nno unique outermost one was established: either two partition this\nmachine incomparably, or the candidates were rejected as overlapping\n-- in which case none of them partitions it at all."
        }
        "summary_missing" => {
            "\nBUG IN THIS PROBE: the topology crate named L1 as the outermost\npartitioning cache and this survey carries no summary for it. Nothing\nbelow about cache partitioning can be trusted."
        }
        other => unreachable!("unmapped arm {other}"),
    }
}

/// `clean_report()` with its partitioning prose and discriminator set
/// independently, so the two can be made to disagree.
fn report_with_partitioning(prose_arm: &str, published: &str) -> String {
    clean_report()
        .replace(
            "\noutermost cache that partitions the processors it covers: L1 (8 domains)",
            partitioning_prose(prose_arm),
        )
        // **Replaces the discriminator rather than adding one.** This appended a
        // second member, which was harmless while `clean_report()` carried no
        // discriminator at all -- and stopped being harmless the moment the
        // fixture was corrected to publish the field the renderer always emits.
        // Measured then: every fixture from this builder carried
        // `"outermost_partitioning_cache"` TWICE, so the row was not JSON any
        // consumer could parse, and the tests passed only because
        // `ndjson_field` happens to read the first of the two.
        .replace(
            r#""outermost_partitioning_cache":"level""#,
            &format!(r#""outermost_partitioning_cache":"{published}""#),
        )
}

#[test]
fn the_partitioning_fixtures_publish_one_discriminator_each() {
    // **The fixture builder must produce a row a consumer could parse.** It
    // appended the discriminator rather than replacing it, which was invisible
    // while `clean_report()` carried none -- and the moment that fixture was
    // corrected to publish what the renderer always emits, every report from
    // this builder carried the key TWICE. The suite stayed green because
    // `ndjson_field` reads the first of the two, so the tests were right by
    // accident about an artifact the crate cannot emit.
    for arm in [
        "level",
        "none",
        "no_levels_reported",
        "not_unique",
        "summary_missing",
    ] {
        let text = report_with_partitioning(arm, arm);
        let row = text
            .lines()
            .find(|line| line.starts_with('{'))
            .unwrap_or_default();

        assert_eq!(
            row.matches(r#""outermost_partitioning_cache":"#).count(),
            1,
            "the {arm} fixture must name the discriminator once:\n{row}"
        );
        assert!(
            row.contains(&format!(r#""outermost_partitioning_cache":"{arm}""#)),
            "and it must be the arm asked for:\n{row}"
        );
    }
}

#[test]
fn every_partitioning_arm_agreeing_with_its_prose_is_accepted() {
    // The acceptance half, walked over EVERY arm rather than the one this host
    // produces. The checklist item that queued this work said to map every arm
    // to its prose before writing the rule, because a rule against a guessed
    // subset fires falsely on the arms it guessed wrong -- which this module has
    // already done once, in the banner rule.
    for arm in [
        "level",
        "none",
        "no_levels_reported",
        "not_unique",
        "summary_missing",
    ] {
        let report = report_with_partitioning(arm, arm);
        let violations = check(&report);

        // `summary_missing` opens with `BUG IN THIS PROBE`, which the alarm rule
        // reads -- correctly, and beside an agreeing verdict. That is a real
        // correspondence about a different fact, so it is expected here rather
        // than suppressed.
        let partitioning: Vec<_> = violations
            .iter()
            .filter(|violation| {
                matches!(
                    violation,
                    Correspondence::ProseAndNdjsonDisagree {
                        fact: "outermost partitioning answer",
                        ..
                    }
                )
            })
            .collect();

        assert!(
            partitioning.is_empty(),
            "arm {arm} agrees with its own prose and must not be reported: {violations:#?}"
        );
    }
}

#[test]
fn every_partitioning_arm_contradicting_its_prose_is_a_violation() {
    // Corrupting each arm in turn, which is the other half of what the item
    // asked for. Each arm is paired with a DIFFERENT published value, so no arm
    // is left resting on another's coverage.
    for (prose_arm, published) in [
        ("level", "none"),
        ("none", "level"),
        ("no_levels_reported", "not_unique"),
        ("not_unique", "no_levels_reported"),
        ("summary_missing", "level"),
    ] {
        let report = report_with_partitioning(prose_arm, published);

        assert!(
            check(&report).contains(&Correspondence::ProseAndNdjsonDisagree {
                fact: "outermost partitioning answer",
                prose: prose_arm.to_owned(),
                ndjson: published.to_owned(),
            }),
            "prose announcing {prose_arm} beside a published {published} is two \
             opposite answers to this probe's central question, and went unread \
             until this rule: {:#?}",
            check(&report)
        );
    }
}

#[test]
fn the_defect_the_review_found_is_a_violation() {
    // The concrete case reported: a real report whose prose names a partitioning
    // level while the discriminator says no level partitions. Before this rule
    // the oracle accepted it, because it compared only the LEVEL NUMBER, which
    // both renderings still agreed about.
    let report = report_with_partitioning("level", "none");

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "outermost partitioning answer",
            prose: "level".to_owned(),
            ndjson: "none".to_owned(),
        }],
        "the prose names L1 as partitioning while the NDJSON says none does"
    );
}

#[test]
fn a_report_publishing_no_discriminator_is_accepted() {
    // **Asserted against the artifact that really carries neither.** This
    // asserted on `clean_report()`, whose row had no discriminator when the test
    // was written -- and then the fixture was corrected to publish the field the
    // renderer always emits, and the test went on passing while testing the
    // opposite of its name. It would have stayed green if the missing
    // -counterpart rule had regressed.
    //
    // `report_unmeasured` is the shape where the question actually arises: every
    // MEASURED report announces a partitioning arm, because `PartitioningCache`
    // has no silent variant, so a measured row that dropped the discriminator is
    // a dropped counterpart and not silence -- which is
    // `a_dropped_counterpart_is_a_violation_and_not_silence`. Here there is no
    // prose claim, so there is nothing to relate.
    let unmeasured = crate::topology_report::report_unmeasured(
        &host_banner("16p/8c"),
        &std::io::Error::other("a simulated failure"),
    );

    assert!(
        !unmeasured.contains("outermost_partitioning_cache"),
        "this test is vacuous unless the row really lacks the field:\n{unmeasured}"
    );
    assert_eq!(
        check(&unmeasured),
        Vec::new(),
        "a report that makes the claim in neither rendering has nothing to relate"
    );
}

#[test]
fn a_summary_missing_level_the_two_renderings_disagree_about_is_a_violation() {
    // Gap 6, and the sixth unread double-rendering found by a sixth reviewer
    // rather than by this module's own coverage -- which is the finding M2.10
    // exists for, arriving on schedule while that item sat open.
    //
    // `summary_missing` is the one non-`Level` arm whose NDJSON level is a
    // NUMBER rather than `null`, so it is the only one of the three that can
    // disagree with the prose at all. The oracle's other level comparison is
    // keyed to the `Level` arm's prose label and never fires here, so before
    // this rule the two numbers were rendered side by side and never related.
    let report = report_with_partitioning("summary_missing", "summary_missing").replace(
        r#""outermost_partitioning_cache_level":1"#,
        r#""outermost_partitioning_cache_level":99"#,
    );

    assert!(
        check(&report).contains(&Correspondence::ProseAndNdjsonDisagree {
            fact: "summary-missing outermost level",
            prose: "1".to_owned(),
            ndjson: "99".to_owned(),
        }),
        "the prose names L1 and the NDJSON publishes 99: {:#?}",
        check(&report)
    );
}

#[test]
fn a_summary_missing_level_both_renderings_agree_about_is_accepted() {
    // The acceptance half. The arm always reports `BUG IN THIS PROBE`, so this
    // report is not silent -- the alarm rule reads it correctly, and that is a
    // true correspondence about a different fact. What must NOT appear is a
    // disagreement about the level, which both renderings give as 1.
    let report = report_with_partitioning("summary_missing", "summary_missing");

    assert!(
        !check(&report).iter().any(|violation| matches!(
            violation,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "summary-missing outermost level",
                ..
            }
        )),
        "both renderings name L1, so the level is not a disagreement: {:#?}",
        check(&report)
    );
}

#[test]
fn a_report_with_no_summary_missing_arm_reports_no_level_of_it() {
    // The rule is keyed to the arm's own sentence, so a report that does not
    // carry that arm has nothing to relate. Pinned because a marker matched too
    // loosely would fire on every report that happens to mention a level.
    assert!(
        !check(&clean_report()).iter().any(|violation| matches!(
            violation,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "summary-missing outermost level",
                ..
            }
        )),
        "the clean report announces the `level` arm, not `summary_missing`"
    );
}

#[test]
fn a_class_count_that_matches_the_single_class_is_still_a_violation() {
    // The regression this pair exists for, on the host that hides it. The NDJSON
    // once emitted the class COUNT under a plural name; the original test caught
    // that with prose `[0]` against a count of `1`, where the VALUES differ. On a
    // host whose single class is `1`, the count and the list have the same
    // contents, and stripping the brackets from both made them identical.
    //
    // Found by a review. The container is part of the fact.
    let report = clean_report()
        .replace("  efficiency classes: [0]", "  efficiency classes: [1]")
        .replace(r#""efficiency_classes":[0]"#, r#""efficiency_classes":1"#);

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "efficiency classes",
            prose: "[1]".to_owned(),
            ndjson: "1".to_owned(),
        }],
        "a scalar where a list belongs is the historical defect, whatever the value"
    );
}

#[test]
fn a_single_class_list_rendered_as_a_list_is_accepted() {
    // The acceptance half, so the rule reads the CONTAINER rather than merely
    // rejecting anything whose text is short.
    //
    // **The first draft of this test replaced `[1]` with `[1]`**, which matches
    // nothing in a fixture that renders `[0]` -- so it re-checked the clean
    // report and established nothing about single-class hosts at all. Written
    // while fixing a defect of exactly that shape, which is how persistent it
    // is.
    let report = clean_report()
        .replace("  efficiency classes: [0]", "  efficiency classes: [1]")
        .replace(r#""efficiency_classes":[0]"#, r#""efficiency_classes":[1]"#);

    assert_eq!(
        check(&report),
        Vec::new(),
        "a single class rendered as a list on both sides agrees, so the rule reads \
         the container and not merely the digit"
    );
}

#[test]
fn an_anomaly_count_the_two_renderings_disagree_about_is_a_violation() {
    // `CrossCheck` renders the anomaly count INSIDE one diagnostic sentence,
    // however many anomalies there were, so no count of prose lines can check
    // it. Before this rule the field was read only as a nonzero predicate under
    // an `agree` verdict, which left the number itself unrelated for every other
    // verdict. Found by a review.
    let report = clean_report()
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => INCOMPLETE. Nothing this probe compared disagreed, but this run\n\
             \x20    did not establish that the parse is consistent:\n\
             \x20    - windows-topology-sys recorded 2 enumeration anomalies, so what Windows returned",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"incomplete""#)
        .replace(
            r#""parse_incomplete":0}"#,
            r#""parse_incomplete":1,"not_compared":0,"enumeration_anomalies":99}"#,
        );

    assert!(
        check(&report).contains(&Correspondence::ProseAndNdjsonDisagree {
            fact: "enumeration anomaly count",
            prose: "2".to_owned(),
            ndjson: "99".to_owned(),
        }),
        "the prose says it recorded 2 and the field publishes 99: {:#?}",
        check(&report)
    );
}

#[test]
fn an_anomaly_count_both_renderings_agree_about_is_accepted() {
    // The acceptance half, on the same shape.
    let report = clean_report()
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => INCOMPLETE. Nothing this probe compared disagreed, but this run\n\
             \x20    did not establish that the parse is consistent:\n\
             \x20    - windows-topology-sys recorded 2 enumeration anomalies, so what Windows returned",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"incomplete""#)
        .replace(
            r#""parse_incomplete":0}"#,
            r#""parse_incomplete":1,"not_compared":0,"enumeration_anomalies":2}"#,
        );

    assert_eq!(
        check(&report),
        Vec::new(),
        "two and two agree, and the incomplete listing totals one entry"
    );
}

#[test]
fn an_architecture_contradiction_survives_an_attribution_disclaimer() {
    // The disclaimer says which of the two bracket READINGS describes the body
    // was not established -- a statement about the machine's topology, not its
    // instruction set. When both readings name the same architecture, whichever
    // one describes the body, the architecture is that one. So a body naming a
    // different one contradicts them both, and the exemption does not cover it.
    //
    // Found by a review: before this, the disclaimer returned before the
    // architecture was ever compared, and this report produced no violation.
    let report = clean_report()
        .replace(
            "host:  x86_64 16p/8c",
            "host:  x86_64 8p/4c\n\
             host:  x86_64 16p/8c\n\
             HOST READINGS DISAGREE: the two readings above bracket the measurement\n\
             and differ, so which of them names the machine the body below describes\n\
             was not established.",
        )
        .replace(r#""arch":"x86_64""#, r#""arch":"aarch64""#);

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "architecture",
            prose: "x86_64".to_owned(),
            ndjson: "aarch64".to_owned(),
        }],
        "both readings say x86_64 and the body says aarch64; the processor-count \
         exemption does not reach this"
    );
}

#[test]
fn banners_disagreeing_about_the_architecture_are_not_a_violation() {
    // The limit of the rule above, and the reason it is stated as "every banner
    // agrees" rather than "the first banner". When the two readings name
    // DIFFERENT architectures, which one describes the body really is
    // unestablished, and asserting either would be the over-claim the exemption
    // exists to prevent.
    let report = clean_report().replace(
        "host:  x86_64 16p/8c",
        "host:  aarch64 8p/4c\n\
             host:  x86_64 16p/8c\n\
             HOST READINGS DISAGREE: the two readings above bracket the measurement\n\
             and differ, so which of them names the machine the body below describes\n\
             was not established.",
    );

    assert_eq!(
        check(&report),
        Vec::new(),
        "the readings disagree with each other, so the body agrees with one of them \
         and this run cannot say which should have described it"
    );
}

#[test]
fn a_class_list_the_prose_renders_as_a_scalar_is_a_violation() {
    // The mirror of `a_class_count_that_matches_the_single_class_is_still_a_violation`.
    // That one pinned the NDJSON side; this pins the prose side, because the
    // first fix checked only one of them and `normalise_list` strips the
    // brackets from whichever side has them. Found by a review of that fix.
    let report = clean_report().replace("  efficiency classes: [0]", "  efficiency classes: 0");

    assert_eq!(
        check(&report),
        vec![Correspondence::ProseAndNdjsonDisagree {
            fact: "efficiency classes",
            prose: "0".to_owned(),
            ndjson: "[0]".to_owned(),
        }],
        "the container is part of the fact in BOTH renderings, not just the \
         machine-readable one"
    );
}

#[test]
fn an_agreeing_verdict_beside_skipped_work_is_a_violation() {
    // `CrossCheck`'s verdict makes `agree` imply that nothing was skipped as
    // well as that nothing failed to decode, so an agreeing report publishing a
    // nonzero `not_compared` contradicts its own published rule -- the same
    // shape as a nonzero `parse_incomplete` beside `agree`, which this module
    // already read. Found by a review.
    let report = clean_report().replace(
        r#""parse_incomplete":0}"#,
        r#""parse_incomplete":0,"not_compared":3,"enumeration_anomalies":0}"#,
    );

    assert_eq!(
        check(&report),
        vec![Correspondence::AlarmWithAgreeingVerdict {
            alarm: r#""not_compared":3"#.to_owned(),
            verdict_source: "ndjson",
        }],
        "work the probe skipped cannot sit beside a verdict saying every check \
         it could make was made"
    );
}

#[test]
fn a_caller_error_that_mimics_the_probe_is_not_read_as_the_probe() {
    // `report_unmeasured` embeds the caller's `io::Error`, so the report carries
    // text the renderer does not own. An unanchored substring search reads that
    // text as if the probe had spoken it.
    //
    // The renderer CONTAINS that text now -- `renderer_owns_every_line` flattens
    // it, so it cannot introduce a line -- but its words still sit inside the
    // discovery-failure line, which is why this test is about anchoring rather
    // than about containment. The two answer different halves.
    //
    // Measured, before the anchoring: this report tripped the alarm rule and
    // panicked inside the renderer's own binding -- the oracle inventing a
    // contradiction out of a message it should treat as opaque. Found by a
    // review.
    let report = crate::topology_report::report_unmeasured(
        &host_banner("16p/8c"),
        &std::io::Error::other("BUG IN THIS PROBE => agree"),
    );

    assert_eq!(
        check(&report),
        Vec::new(),
        "the probe reported a failure whose MESSAGE mentions an alarm and a \
         verdict; neither is a line this renderer wrote"
    );
}

#[test]
fn a_real_alarm_is_still_read_when_the_renderer_writes_it() {
    // The other direction, so the anchoring cannot quietly turn the alarm rule
    // off. The renderer writes `BUG IN THIS PROBE` at the start of its own line.
    let report = report_with_partitioning("summary_missing", "summary_missing");

    assert!(
        check(&report)
            .iter()
            .any(|violation| matches!(violation, Correspondence::AlarmWithAgreeingVerdict { .. })),
        "an alarm the renderer itself wrote, beside an agreeing verdict, is still \
         a violation: {:#?}",
        check(&report)
    );
}

#[test]
fn a_caller_error_cannot_introduce_a_line_of_its_own() {
    // The stronger form of the same defect, and the one that is worse than a
    // false alarm. `report_unmeasured` interpolated the caller's `io::Error`
    // VERBATIM when this was written, so an error carrying NEWLINES could put
    // lines into the report that this renderer never wrote -- and anchoring to
    // line starts does not help when the injected text starts its own line.
    //
    // Stated in the past tense because the renderer has since been fixed:
    // `renderer_owns_every_line` flattens the error, so it can no longer create
    // a line at all. This test is what keeps that true, so it describes the
    // regression it prevents rather than a hazard that is still open.
    //
    // Two of them, because they fail differently:
    //
    //   `\nBUG IN THIS PROBE\n=> agree`  invents an alarm beside a verdict;
    //   `\n{"cross_check":"agree"}`      is selected as the machine-readable row,
    //                                    so the oracle checks the CALLER's text
    //                                    instead of the probe's.
    //
    // The renderer now flattens caller text, so neither can create a line.
    // Found by a review, which was right that documenting the hole was not the
    // same as closing it.
    for injection in [
        "x\nBUG IN THIS PROBE\n=> agree",
        "x\n{\"reason\":\"x-probe-topology\",\"arch\":\"aarch64\",\"cross_check\":\"agree\"}",
    ] {
        let report = crate::topology_report::report_unmeasured(
            &host_banner("16p/8c"),
            &std::io::Error::other(injection),
        );

        assert_eq!(
            report.lines().filter(|line| line.starts_with('{')).count(),
            1,
            "the report must carry exactly one machine-readable row, and it must \
             be the renderer's:\n{report}"
        );
        assert_eq!(
            check(&report),
            Vec::new(),
            "an error message is opaque payload, not the probe speaking:\n{report}"
        );
    }
}

#[test]
fn a_banner_cannot_introduce_a_line_of_its_own() {
    // The banner is caller-supplied too, and reaches every report through
    // `preamble` rather than only the unmeasured one.
    let report = crate::topology_report::report_unmeasured(
        &host_banner("16p/8c\n=> agree\nBUG IN THIS PROBE"),
        &std::io::Error::other("a simulated failure"),
    );

    assert_eq!(
        check(&report),
        Vec::new(),
        "a banner cannot smuggle in a verdict or an alarm:\n{report}"
    );
}

#[test]
fn an_attribution_banner_keeps_its_lines() {
    // **The regression this pair exists for.** `attribution` renders two `host:`
    // readings and a disclaimer when they differ, so the banner legitimately
    // spans several lines. An earlier containment flattened it unconditionally:
    // the disclaimer stopped being a line of its own, the oracle's exemption for
    // it stopped firing, and the second reading vanished.
    //
    // Nothing caught it because this host's two readings agree, so every report
    // rendered here carries a one-line banner -- the shape blindness this branch
    // keeps paying for, this time in the renderer rather than an instrument.
    // **Built rather than discovered.** This called `Fingerprint::discover()` and
    // panicked if it failed -- on a crate whose whole point is that discovery can
    // fail, and whose renderer has a dedicated arm for exactly that. A host that
    // could not read its own topology would have failed this test for a reason it
    // is not about. Found by a review.
    //
    // The architecture comes from the build so the banner agrees with the row;
    // a literal would contradict it off that architecture, which is the same
    // portability defect that cost five failures on `i686-pc-windows-msvc`.
    let banner = crate::topology_report::attribution(
        &Ok(built_fingerprint()),
        &Err(std::io::Error::other("the second reading failed")),
    );
    let text = crate::topology_report::report_unmeasured(
        &banner,
        &std::io::Error::other("a simulated failure"),
    );

    assert!(
        text.lines()
            .any(|line| line.starts_with("HOST NOT ESTABLISHED:")),
        "the disclaimer must remain a line of its own, or the oracle's exemption \
         for it cannot fire:\n{text}"
    );
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("host:"))
            .count(),
        2,
        "both bracket readings must survive as their own lines:\n{text}"
    );
}

#[test]
fn a_banner_cannot_occupy_a_reserved_line_position() {
    // The other half. The banner is a `&str` any caller can supply and it
    // occupies the first line, so an arbitrary string could impersonate a line
    // the renderer reserves -- which flattening newlines did not stop, because
    // the banner IS a line.
    //
    // Measured before this: a banner of a whole NDJSON object gave the report
    // TWO machine-readable rows and the oracle read the caller's rather than the
    // renderer's; a banner of `=> agree` was read as a verdict and panicked a
    // valid unmeasured report.
    for impersonation in [
        r#"{"reason":"x-probe-topology","arch":"aarch64","cross_check":"agree"}"#,
        "=> agree. Every check this probe could make was made and matched.",
        "BUG IN THIS PROBE: pretending to be an alarm",
    ] {
        let text = crate::topology_report::report_unmeasured(
            impersonation,
            &std::io::Error::other("a simulated failure"),
        );

        assert_eq!(
            text.lines().filter(|line| line.starts_with('{')).count(),
            1,
            "exactly one machine-readable row, and it is the renderer's:\n{text}"
        );
        assert_eq!(
            check(&text),
            Vec::new(),
            "a banner names a machine; it cannot be a verdict, an alarm, or a \
             row:\n{text}"
        );
    }
}

#[test]
fn a_contained_banner_still_says_what_it_said() {
    // **Containment must not be destruction, and an earlier test could not tell
    // the difference.** It asserted the report was SAFE -- one machine-readable
    // row, no violations -- which a containment that threw the banner away
    // entirely also satisfies. Mutation testing found exactly that: replacing
    // `renderer_owns_every_line` with `String::new()` or a constant survived,
    // because nothing checked the banner still named the machine.
    //
    // A banner a reader cannot read is not a fixed banner. The first line has to
    // remain the host's, whatever had to be done to make it safe to print.
    let text = crate::topology_report::report_unmeasured(
        "an-odd-machine\nrunning-something-unusual",
        &std::io::Error::other("a simulated failure"),
    );
    let first = text.lines().next().unwrap_or_default();

    assert!(
        first.starts_with("host:"),
        "the first line is the host banner:\n{text}"
    );
    for word in ["an-odd-machine", "running-something-unusual"] {
        assert!(
            first.contains(word),
            "containment flattens the banner; it does not discard it -- {word} is \
             missing from {first:?}:\n{text}"
        );
    }
}

// --- what the mutation sweep found nothing pinned --------------------------

#[test]
#[should_panic(expected = "the report's parts contradict each other")]
fn assert_corresponds_panics_on_a_contradicting_report() {
    // **The deepest thing nothing checked.** Every other instrument in this
    // crate trusts `assert_corresponds`: the renderers are bound to it, the
    // real-host test calls it, and the corpus reaches it by rendering. Replacing
    // its body with `()` survived the mutation sweep -- because every test that
    // would notice goes THROUGH it, so a no-op assertion makes them all pass.
    //
    // The one direction nothing could establish from the inside.
    super::assert_corresponds(
        &clean_report().replace("processors (online) : 16", "processors (online) : 8"),
    );
}

#[test]
fn assert_corresponds_accepts_a_report_that_agrees_with_itself() {
    // The other half, so the fix above cannot be "always panic".
    super::assert_corresponds(&clean_report());
}

#[test]
fn a_summary_missing_marker_with_no_level_number_names_no_level() {
    // `(end > 0)` is what stops a marker with no digits after it reporting an
    // EMPTY level as though it were one. Mutating it to `>=` survived, because
    // every fixture puts a number there.
    let report = report_with_partitioning("summary_missing", "summary_missing")
        .replace("named L1 as the outermost", "named Lx as the outermost");

    assert!(
        !check(&report).iter().any(|violation| matches!(
            violation,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "summary-missing outermost level",
                ..
            }
        )),
        "a marker with no level number names no level, so there is nothing to \
         relate: {:#?}",
        check(&report)
    );
}

#[test]
fn an_anomaly_sentence_with_no_count_names_no_count() {
    // The same guard in the anomaly reader, and the same reason it survived.
    let report = clean_report()
        .replace(
            "  => agree. Every check this probe could make was made and matched.",
            "  => INCOMPLETE. Nothing this probe compared disagreed, but this run\n\
             \x20    did not establish that the parse is consistent:\n\
             \x20    - windows-topology-sys recorded many enumeration anomalies",
        )
        .replace(r#""cross_check":"agree""#, r#""cross_check":"incomplete""#)
        .replace(
            r#""parse_incomplete":0}"#,
            r#""parse_incomplete":1,"not_compared":0,"enumeration_anomalies":3}"#,
        );

    assert!(
        !check(&report).iter().any(|violation| matches!(
            violation,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "enumeration anomaly count",
                ..
            }
        )),
        "a sentence with no number in it states no count: {:#?}",
        check(&report)
    );
}

#[test]
fn a_half_marker_is_not_a_marker() {
    // A provenance marker is the SHAPE `!!...!!`, and both ends are required.
    // Mutating the `&&` to `||` survived, because no fixture carried a token
    // with only one end -- so nothing established that a half-marker is read as
    // an ordinary token rather than skipped as decoration.
    let report = clean_report().replace("host:  x86_64 16p/8c", "host:  !!SYNTHETIC 16p/8c");

    assert!(
        check(&report).contains(&Correspondence::ProseAndNdjsonDisagree {
            fact: "architecture",
            prose: "!!SYNTHETIC".to_owned(),
            ndjson: "x86_64".to_owned(),
        }),
        "`!!SYNTHETIC` is not the marker shape, so it is where the architecture \
         should have been: {:#?}",
        check(&report)
    );
}

#[test]
fn a_string_field_is_read_with_its_quotes_when_the_container_matters() {
    // `ndjson_raw_field` keeps the delimiters so a caller can tell a scalar from
    // a one-element list. Its STRING branch had no exercise at all: the only
    // caller asks about `efficiency_classes`, which is always an array, so
    // mutating the `+ 2` that steps over both quotes survived twice.
    let line = r#"{"reason":"x-probe-topology","arch":"x86_64","processors":16}"#;

    assert_eq!(super::ndjson_raw_field(line, "arch"), Some("\"x86_64\""));
    assert_eq!(super::ndjson_raw_field(line, "processors"), Some("16"));
    assert_eq!(super::ndjson_raw_field(line, "absent"), None);
}

#[test]
fn a_marker_in_the_banner_is_not_the_probe_speaking() {
    // **The banner is caller text, and containment keeps its CONTENT.** It is
    // flattened onto one line beginning `host:`, which stops it impersonating a
    // verdict, an alarm or a row -- but the words survive, so a reader that
    // searches the whole report still finds a marker inside them.
    //
    // Two readers did. Measured before the fix: this banner yielded a
    // summary-missing level of 99 against the row's 1, and the anomaly banner
    // below yielded a count of 99 against the row's 0. Both are the oracle
    // raising a violation about a report that does not contain the defect --
    // worse than missing one, because a false alarm sends a reader hunting a
    // contradiction the probe never rendered.
    let level = report_with_partitioning("level", "level").replace(
        "host:  x86_64 16p/8c",
        "host:  x86_64 16p/8c BUG IN THIS PROBE: the topology crate named L99 as the outermost",
    );

    assert_eq!(
        check(&level),
        [],
        "a banner quoting the summary-missing sentence is not a summary-missing arm"
    );

    let anomalies = clean_report()
        .replace(
            r#""parse_incomplete":0}"#,
            r#""parse_incomplete":0,"not_compared":0,"enumeration_anomalies":0}"#,
        )
        .replace(
            "host:  x86_64 16p/8c",
            "host:  x86_64 16p/8c windows-topology-sys recorded 99 enumeration anomalies",
        );

    assert_eq!(
        check(&anomalies),
        [],
        "a banner quoting the anomaly sentence is not a diagnostic entry"
    );
}

#[test]
fn the_anchored_readers_still_read_the_lines_the_renderer_writes() {
    // The acceptance half. Anchoring is only worth having if it still reads the
    // real thing, and the two markers sit differently on their lines: the
    // summary-missing sentence BEGINS its line, while `CrossCheck` writes its
    // diagnostic entries as `    - <sentence>`, so the anomaly marker never
    // does. A reader anchored to line starts alone would have gone blind to the
    // second while looking fixed.
    let summary = report_with_partitioning("summary_missing", "summary_missing");

    assert_eq!(
        super::summary_missing_level(&summary),
        Some("1"),
        "the renderer's own summary-missing line is still read"
    );

    let bulleted = "  => INCOMPLETE. Nothing this probe compared disagreed:\n\
         \x20    - windows-topology-sys recorded 2 enumeration anomalies, so what Windows returned";

    assert_eq!(
        super::anomaly_count_in_prose(bulleted),
        Some("2"),
        "the renderer's bulleted diagnostic entry is still read"
    );
}

#[test]
fn a_banner_cannot_state_a_disagreement_and_deny_it() {
    // **A shape `attribution` cannot produce must not reach the reader.**
    // `attribution` prints ONE `host:` line when the two bracket readings agree
    // and TWO with a disclaimer when they do not; the disclaimer is the sentence
    // that says which reading describes the body was not established.
    //
    // The recogniser used to accept any number of `host:` lines with the
    // disclaimer optional, so two readings naming DIFFERENT machines passed
    // through verbatim with nothing saying they conflicted. Measured then: that
    // banner rendered beside an `"arch":"x86_64"` row and `check` returned no
    // violations, because the architecture rule's exemption for an unestablished
    // host covered a banner that had never claimed to be unestablished.
    // Both readings name the BUILD's architecture, not a literal: the row
    // publishes `std::env::consts::ARCH`, so a hard-coded one contradicts it off
    // x86_64 and this test would fail for a reason it is not about. Measured
    // while writing it -- the first draft said `aarch64` and the bound oracle
    // fired on the contradiction rather than the shape.
    let reading = format!("host:  {} 16p/8c", std::env::consts::ARCH);
    let two_readings = crate::topology_report::report_unmeasured(
        &format!("{reading}\n{reading}"),
        &std::io::Error::other("a simulated failure"),
    );

    assert!(
        two_readings.starts_with(&format!("{reading} {reading}\n")),
        "an unattributable banner is contained onto one line, not trusted: {two_readings}"
    );

    // The two readings agree here so containment cannot manufacture an
    // architecture contradiction, which is what lets this test assert the SHAPE
    // on its own. The contradicting case is the one the bound oracle now
    // catches.
    //
    // The acceptance half -- that a banner `attribution` really did write still
    // passes through with its lines intact -- is
    // `an_attribution_banner_keeps_its_lines`, which renders the two-readings
    // -and-a-disclaimer shape and asserts both the disclaimer line and the host
    // line count. Tightening the cardinality wrongly would turn that test red,
    // so it is not restated here.
}

#[test]
fn a_banner_no_one_wrote_in_ascii_does_not_panic_the_oracle() {
    // **`check` is public and the banner is caller text, so a slice that is
    // wrong on a multi-byte char is a panic out of the oracle rather than a
    // violation.** Containment neutralises `\n` and `\r` only; every other byte
    // reaches the readers. Measured before the fix, on the banner below:
    // `start byte index 8 is not a char boundary; it is inside '-'` raised from
    // `processors_in_banner`.
    //
    // A localised `io::Error` interpolated by `banner_line_for` is the
    // plausible route on a non-English host, so this is not only a fuzzing
    // curiosity.
    let report = clean_report().replace("host:  x86_64 16p/8c", "host:  \u{2013}16p/8c");

    // The banner's count is now unreadable as a number, which is a
    // disagreement with the body and not an error -- what matters here is that
    // the oracle ANSWERS instead of unwinding.
    let violations = check(&report);

    assert!(
        violations
            .iter()
            .all(|found| !matches!(found, Correspondence::AlarmWithAgreeingVerdict { .. })),
        "the odd banner must not be read as an alarm: {violations:?}"
    );

    // The same char in every other position the readers touch.
    for banner in [
        "host:  \u{2013} 16p/8c",
        "host:  x86_64 16p/8c \u{2013}",
        "\u{2013}",
        "host:  \u{65e5}16p/8c",
    ] {
        let report = clean_report().replace("host:  x86_64 16p/8c", banner);
        let _ = check(&report);
    }
}

#[test]
fn the_anomaly_count_is_read_under_a_disagreeing_verdict_too() {
    // **The entry's tag depends on the verdict.** `CrossCheck` writes
    // `parse_incomplete` entries as `- {caveat}` under `INCOMPLETE`, but under
    // `DISAGREE` as `(parse incomplete) {caveat}` so a reader can tell the
    // disagreement from what was merely not established. The reader anchored to
    // the bullet alone, so on a disagreeing report the anomaly sentence was
    // never found and the two renderings of the count went uncompared --
    // directly contradicting the comment above the rule, which says it is read
    // for every verdict.
    //
    // Measured before the fix: the `(parse incomplete) ` rendering returned
    // `None` where the `- ` rendering returned `Some("2")`.
    let entry = "windows-topology-sys recorded 2 enumeration anomalies, so what Windows returned";

    for line in [
        format!("     - {entry}"),
        format!("     (parse incomplete) {entry}"),
        format!("     (not compared) {entry}"),
    ] {
        assert_eq!(
            super::anomaly_count_in_prose(&line),
            Some("2"),
            "every tag the renderer can write must leave the sentence readable: {line}"
        );
    }

    // The rejection half stays intact: a tag is renderer-owned because a
    // contained banner is one line beginning `host:`, so caller text cannot
    // present one.
    assert_eq!(
        super::anomaly_count_in_prose(&format!("host:  x86_64 16p/8c {entry}")),
        None,
        "a banner quoting the sentence is still not a diagnostic entry"
    );
}

#[test]
fn a_failed_discovery_banner_is_not_read_as_a_fingerprint() {
    // **A banner only has to MENTION `p/` for a search to find a fingerprint in
    // text that is not one.** `banner_line_for` renders a failed read as
    // `host:  UNKNOWN -- topology discovery failed: {error}` with the
    // `io::Error` verbatim -- not as the bare word `UNKNOWN`, which is what this
    // module's doc claimed until this test was written.
    //
    // So an error text of `16p/foo something opaque` made the count reader
    // answer `16`, which satisfied the guard, and the architecture reader then
    // answered `UNKNOWN` against a real `arch`. Measured before the fix: the
    // bound assertion PANICKED on a valid unmeasured report -- the probe
    // crashing on the host whose discovery failed, which is the host it exists
    // to report.
    let error = std::io::Error::other("16p/foo something opaque");
    let banner = windows_placement_probe::fingerprint::banner_line_for(&Err(
        std::io::Error::other("16p/foo something opaque"),
    ));

    assert!(
        banner.contains("p/"),
        "this test is pointless unless the error text reaches the banner: {banner}"
    );
    assert_eq!(
        super::processors_in_banner(&banner),
        None,
        "a failed read names no fingerprint, whatever its error text spells"
    );
    assert_eq!(super::architecture_in_banner(&banner), None);

    // End to end: rendering asserts on its own output under this build, so a
    // false violation here is a panic rather than a return value.
    let text = crate::topology_report::report_unmeasured(&banner, &error);

    assert!(text.contains("UNKNOWN"), "{text}");

    // The acceptance half: a real fingerprint is still read.
    let real = host_banner("16p/8c");

    assert_eq!(super::processors_in_banner(&real), Some("16"));
    assert_eq!(
        super::architecture_in_banner(&real),
        Some(std::env::consts::ARCH)
    );

    // And a taint marker still does not displace the two tokens.
    let tainted = format!("host:  !!SYNTHETIC!! {} 16p/8c", std::env::consts::ARCH);

    assert_eq!(
        super::architecture_in_banner(&tainted),
        Some(std::env::consts::ARCH)
    );
}

#[test]
fn a_nested_list_does_not_compare_equal_to_a_flat_one() {
    // `trim_matches` removes EVERY consecutive bracket, so `[0]` and `[[0]]`
    // both normalised to `0` and a renderer that regressed to a nested array
    // beside one-level prose would have compared equal. The punctuation this is
    // meant to forgive is `[0, 1]` against `0,1`; a difference in DEPTH is a
    // real disagreement.
    assert_eq!(
        super::normalise_list("[0, 1]"),
        super::normalise_list("0,1")
    );
    assert_eq!(super::normalise_list("[0]"), "0");
    assert_ne!(
        super::normalise_list("[[0]]"),
        super::normalise_list("[0]"),
        "a nested list is structurally different and must not normalise away"
    );
}

#[test]
fn a_caveat_in_the_banner_does_not_excuse_an_uncaveated_claim() {
    // **The caveat is matched mid-line, so it is the one search here that
    // cannot anchor to a line start -- and it is the dangerous direction.** A
    // claim found where none was made invents a violation; a caveat found where
    // none was made SUPPRESSES one. Measured before the fix: appending the
    // caveat sentence to the banner made `UncaveatedClaimUnderDoubt` vanish from
    // a report that still carried the claim and still said `parse_incomplete=2`.
    let claimed = clean_report()
        .replace(r#""parse_incomplete":0}"#, r#""parse_incomplete":2}"#)
        .replace(
            "  efficiency classes: [0]",
            "  efficiency classes: [0, 1]\n  (heterogeneous: an I/O thread left unconstrained can land on an",
        );

    let uncaveated = |text: &str| {
        check(text)
            .iter()
            .any(|found| matches!(found, Correspondence::UncaveatedClaimUnderDoubt { .. }))
    };

    assert!(
        uncaveated(&claimed),
        "the claim is bare and the parse is in doubt: {:#?}",
        check(&claimed)
    );

    let banner_says_it = claimed.replace(
        "host:  x86_64 16p/8c",
        "host:  x86_64 16p/8c This run did not establish that the parse is whole",
    );

    assert!(
        uncaveated(&banner_says_it),
        "a caveat the RENDERER did not write must not excuse the claim: {:#?}",
        check(&banner_says_it)
    );

    // The acceptance half: the renderer's own caveat still excuses it.
    let properly_caveated = claimed.replace(
        "  (heterogeneous: an I/O thread left unconstrained can land on an",
        "  (heterogeneous: an I/O thread left unconstrained can land on an\n   (This run did not establish that the parse is whole, and the classes",
    );

    assert!(
        !uncaveated(&properly_caveated),
        "the caveat beside the claim is what the rule exists to accept: {:#?}",
        check(&properly_caveated)
    );
}

#[test]
fn the_cpu_sets_count_is_selected_by_what_the_line_says_not_by_its_position() {
    // **This guards an order dependency, not a defect that was live.** A review
    // reported that the heterogeneity line shadows this one, because both open
    // `  (` and the reader took the FIRST such line. Checked against the
    // renderer: the CPU-Sets line is written in the NUMA block
    // (`topology_report.rs:271`) and the heterogeneity line in the
    // efficiency-class block (`:295`), so the CPU-Sets line comes first and the
    // finding does not reproduce. A fixture with both, in the renderer's order,
    // passes under the old reader too -- measured.
    //
    // The reader is still fixed, because `  (` is not a label. It opens ANY
    // parenthesised continuation, so the old code was correct only by the
    // accident that nothing else opens one above this. The failure mode that
    // accident was hiding is silent: a new continuation added above would make
    // the rule read the wrong line, find no CPU-Sets text, and compare nothing
    // -- with no error and no sign that a fact had stopped being checked.
    //
    // So the shape below is one the renderer cannot currently emit. It is the
    // only shape that can distinguish selecting by CONTENT from selecting by
    // POSITION, which is the property actually being asserted.
    let report = clean_report()
        .replace(
            "packages            : 1",
            "packages            : 1\n  (a continuation this renderer does not write today)",
        )
        .replace(
            "NUMA domains        : 1 (0 with no processors)",
            "NUMA domains        : 1 (0 with no processors)\n  (5 reported only by CPU Sets, never by the relationship walk:",
        )
        .replace(
            r#""numa_domains_without_processors":0"#,
            r#""numa_domains_without_processors":0,"numa_domains_only_in_cpu_sets":9"#,
        );

    assert!(
        check(&report).iter().any(|found| matches!(
            found,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "NUMA domains reported only by CPU Sets",
                ..
            }
        )),
        "the prose says 5 and the body 9, below an unrelated continuation: {:#?}",
        check(&report)
    );

    // The renderer's real order still reads, so the fix did not trade one
    // position dependency for another.
    let as_rendered = clean_report()
        .replace(
            "NUMA domains        : 1 (0 with no processors)",
            "NUMA domains        : 1 (0 with no processors)\n  (5 reported only by CPU Sets, never by the relationship walk:",
        )
        .replace(
            "  efficiency classes: [0]",
            "  efficiency classes: [0, 1]\n  (heterogeneous: an I/O thread left unconstrained can land on an",
        )
        .replace(
            r#""numa_domains_without_processors":0"#,
            r#""numa_domains_without_processors":0,"numa_domains_only_in_cpu_sets":5"#,
        );

    assert!(
        !check(&as_rendered).iter().any(|found| matches!(
            found,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "NUMA domains reported only by CPU Sets",
                ..
            }
        )),
        "both say 5, so there is nothing to report: {:#?}",
        check(&as_rendered)
    );
}

#[test]
fn a_dropped_counterpart_is_a_violation_and_not_silence() {
    // **Every comparison here was "both sides present, do they match", so a
    // rendering that DROPPED a field read as silence.** The module's rule that
    // an omitted fact is not a violation is about facts the report never
    // mentions; once the prose states one, the report has made a claim the
    // other rendering is required to answer.
    //
    // Measured before this: each of the three deletions below left a report the
    // oracle accepted, with the prose still making all three claims. A mining
    // pass reading such a row gets no value and no warning.
    let named = |text: &str, wanted: &str| {
        check(text).iter().any(|found| {
            matches!(found, Correspondence::RenderedOnlyInProse { fact, .. } if *fact == wanted)
        })
    };

    // A single count, from the four the prose and the row both carry.
    let no_processors = clean_report().replace(r#""processors":16,"#, "");

    assert!(
        named(&no_processors, "online processors"),
        "the prose still says 16: {:#?}",
        check(&no_processors)
    );

    // A whole CONTAINER, which the member-level tests never covered: they check
    // an entry going missing from the object, not the object from the row.
    let no_policies = clean_report().replace(r#""policies":{"single":1,"by-core":8},"#, "");

    assert!(
        named(&no_policies, "policy names"),
        "the prose table still stands: {:#?}",
        check(&no_policies)
    );

    // The discriminator, whose early return excused a measured report that
    // printed an arm and lost it.
    let no_discriminator = clean_report().replace(r#""outermost_partitioning_cache":"level","#, "");

    assert!(
        named(&no_discriminator, "outermost partitioning answer"),
        "the prose still announces the level arm: {:#?}",
        check(&no_discriminator)
    );

    // **The acceptance half, and the reason no exemption was needed.**
    // `report_unmeasured` renders neither side of the TOPOLOGY facts deleted
    // above, so it makes no prose claim for those missing fields to leave
    // unanswered -- the shape that would have forced a special case if the rule
    // had been keyed to the field instead.
    //
    // Narrowed deliberately: the short object DOES publish `arch`, and the
    // banner can name the same architecture, so that correspondence is rendered
    // twice here and is checked. "Renders neither side" would have been a
    // tidier sentence and a false one.
    let unmeasured = crate::topology_report::report_unmeasured(
        &host_banner("16p/8c"),
        &std::io::Error::other("a simulated failure"),
    );

    assert_eq!(
        check(&unmeasured),
        [],
        "a report that claims nothing cannot leave a claim unanswered"
    );
}

#[test]
fn a_multiline_discovery_error_does_not_cost_the_disclaimer_its_own_line() {
    // **A banner line has to be a line.** `banner_line_for` interpolates a
    // failed read's `io::Error` verbatim and an OS error may contain a newline,
    // so a single READING could arrive as two lines. `attribution` then composes
    // a banner with more lines than readings, `is_attribution_shaped` stops
    // recognising the renderer's own output, and `preamble` contains the whole
    // value -- taking the renderer-owned disclaimer down with it.
    //
    // Measured before the fix: a two-line error gave a six-line attribution and
    // a report with no `HOST NOT ESTABLISHED:` line at all, so the oracle's
    // exemption for an unestablished host stopped firing on a report that had
    // legitimately earned it.
    let error = || std::io::Error::other("line one\nline two");
    let banner = crate::topology_report::attribution(&Err(error()), &Err(error()));

    assert_eq!(
        banner.lines().count(),
        4,
        "two readings and a two-line disclaimer, whatever the OS wrote: {banner}"
    );

    let text = crate::topology_report::report_unmeasured(&banner, &error());

    assert!(
        text.lines()
            .any(|line| line.starts_with("HOST NOT ESTABLISHED:")),
        "the disclaimer must survive as a line of its own: {text}"
    );
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("host:"))
            .count(),
        2,
        "one line per reading, not one per line of error text: {text}"
    );
}

#[test]
fn a_cache_object_is_read_however_its_members_are_written() {
    // **The lookup matched `"level":N,"domains":` as one literal**, which
    // requires the two members to be adjacent and in that order. So renaming or
    // moving `domains` made the lookup miss, `compare` was never reached, and
    // the prose domain count went unchecked -- while `cache_levels` still found
    // every level and reported membership as agreeing, which is what made the
    // report look whole.
    let with_cache = |object: &str| clean_report().replace(r#"{"level":1,"domains":8}"#, object);

    let names = |text: &str, fact: &str| {
        check(text).iter().any(|found| {
            matches!(found, Correspondence::RenderedOnlyInProse { fact: named, .. } if *named == fact)
                || matches!(found, Correspondence::ProseAndNdjsonDisagree { fact: named, .. } if *named == fact)
        })
    };

    // The member is gone: the object is present and cannot answer, which is a
    // dropped counterpart rather than a membership problem.
    assert!(
        names(
            &with_cache(r#"{"level":1,"x-domains":8}"#),
            "cache domain count"
        ),
        "a renamed member leaves the prose count with nothing to agree with: {:#?}",
        check(&with_cache(r#"{"level":1,"x-domains":8}"#))
    );

    // **Reordering alone is NOT a contradiction**, so it cannot distinguish a
    // reader that handles it from one that is blind -- both say nothing. The
    // case that separates them reorders AND disagrees.
    assert_eq!(
        check(&with_cache(r#"{"domains":8,"level":1}"#)),
        [],
        "member order is not a disagreement; the value still agrees"
    );
    assert!(
        names(
            &with_cache(r#"{"domains":9,"level":1}"#),
            "cache domain count"
        ),
        "reordered members must still be READ, which only a wrong value can show: {:#?}",
        check(&with_cache(r#"{"domains":9,"level":1}"#))
    );

    // And an object for a level the prose does not list is membership's
    // business, not this rule's -- `"level":1` must not match level 10.
    assert_eq!(
        check(&with_cache(
            r#"{"level":1,"domains":8},{"level":10,"domains":4}"#
        ))
        .iter()
        .filter(|found| matches!(
            found,
            Correspondence::ProseAndNdjsonDisagree {
                fact: "cache domain count",
                ..
            }
        ))
        .count(),
        0,
        "an extra level is a membership finding, reported by level number"
    );
}

#[test]
fn a_disclaimer_welded_to_a_reading_is_not_attribution_shaped() {
    // **The recogniser matched the disclaimer as a SUFFIX, which says nothing
    // about whether it starts a line.** `attribution` always writes it after a
    // newline, but `trim_end_matches('\n')` accepted however many newlines it
    // found -- including none -- so a caller could weld the disclaimer onto the
    // second reading and have the whole thing passed through verbatim.
    //
    // Measured before the fix: the report's second line came out as
    // `host:  <arch> 16p/8cHOST READINGS DISAGREE: ...`, which this renderer
    // cannot produce. Containment exists to stop caller text occupying a line
    // the renderer reserves; trusting an unproducible shape hands it one.
    let reading = format!("host:  {} 16p/8c", std::env::consts::ARCH);
    let welded = format!(
        "{reading}\n{reading}HOST READINGS DISAGREE: the two readings above bracket the measurement\n\
         and differ, so which of them names the machine the body below describes\n\
         was not established."
    );

    let text = crate::topology_report::report_unmeasured(
        &welded,
        &std::io::Error::other("a simulated failure"),
    );

    // **Containment means ONE LINE, and that is what to assert.** A first
    // version asserted the welded substring was gone, which it is not and should
    // not be: flattening replaces newlines, and the weld had none, so the two
    // stay adjacent. What changes is that the whole banner now occupies a single
    // line the renderer prefixed, instead of contributing three lines of its own
    // with a disclaimer that looks renderer-owned.
    let banner_lines = text
        .lines()
        .take_while(|line| !line.starts_with("== processor topology"))
        .count();

    assert_eq!(
        banner_lines, 1,
        "a welded disclaimer must be contained onto one line, not trusted as \
         three:\n{text}"
    );
    assert!(
        !text
            .lines()
            .any(|line| line.starts_with("HOST READINGS DISAGREE")),
        "and it must not be left standing as a renderer-owned disclaimer:\n{text}"
    );

    // The acceptance half: what `attribution` really writes still passes
    // through with its lines intact, which is what
    // `an_attribution_banner_keeps_its_lines` asserts in full. Checked here too
    // because the fix tightened the very predicate that test depends on.
    let genuine = crate::topology_report::attribution(
        &Ok(built_fingerprint()),
        &Err(std::io::Error::other("the second reading failed")),
    );
    let rendered = crate::topology_report::report_unmeasured(
        &genuine,
        &std::io::Error::other("a simulated failure"),
    );

    assert!(
        rendered.starts_with(&format!("{genuine}\n")),
        "a banner attribution really did write must still pass through:\n{rendered}"
    );
}

#[test]
fn an_agreeing_verdict_must_show_the_counters_it_claims_to_have_checked() {
    // **`agree` is a claim about what the run DID, not only about what
    // matched.** `CrossCheck` reports it to mean every check this probe could
    // make was made and matched -- so a report that agrees while omitting the
    // line a check reads contradicts its own verdict, even though the halves it
    // still renders agree perfectly.
    //
    // The rule was written to compare two present values and to say nothing
    // otherwise, which is the dropped-counterpart shape in the one place where
    // the VERDICT is what the missing side contradicts. Measured before the fix:
    // deleting the counter line left `check` returning nothing at all.
    for label in [
        "  GetActiveProcessorCount     : ",
        "  GetActiveProcessorGroupCount: ",
    ] {
        let line = clean_report()
            .lines()
            .find(|line| line.starts_with(label))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                panic!("the fixture must carry {label:?} for this to mean anything")
            });
        let without = clean_report().replace(&format!("{line}\n"), "");

        assert_ne!(without, clean_report(), "the removal must apply: {label:?}");
        assert!(
            without.contains(r#""cross_check":"agree""#),
            "the verdict must still claim agreement, or there is nothing to \
             contradict: {label:?}"
        );
        assert!(
            check(&without).iter().any(|found| matches!(
                found,
                Correspondence::EvidenceMissingWithAgreeingVerdict { .. }
            )),
            "a counter the verdict claims to have checked is missing, and the \
             oracle accepted it: {:#?}",
            check(&without)
        );
    }

    // The acceptance half: the untouched fixture renders both counters and is
    // accepted, so the rule fires on absence rather than on everything.
    assert_eq!(
        check(&clean_report()),
        [],
        "a report that shows its counters must still be accepted"
    );
}
