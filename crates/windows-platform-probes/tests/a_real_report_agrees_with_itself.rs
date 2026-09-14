// Copyright (c) Mike Grier.

//! The report this crate renders from a *real* measurement, checked against the
//! oracle.
//!
//! # Why this is not a unit test
//!
//! The oracle's own tests pin it against fixtures. A fixture is a report
//! somebody wrote down, so a fixture-bound oracle checks correspondences over
//! states its author already imagined -- and the defect the oracle exists for
//! was a state nobody had imagined: `topology_report` printing `BUG IN THIS
//! PROBE ... Nothing below about cache partitioning can be trusted` while the
//! verdict two paragraphs below printed `=> agree`.
//!
//! More narrowly, a fixture cannot notice the **renderer** drifting away from
//! the prose labels the oracle looks for. Both sides would still agree with
//! each other; only the real artifact disagrees.
//!
//! Some unit tests in this crate do call `measure()` and so do read this host.
//! What none of them does is run the ORACLE over a report rendered from that
//! reading, which is the gap this file closes. On CI it runs across the hosted
//! runner fleet -- a slow survey of shapes no fixture anticipates.
//!
//! # It asserts nothing about this machine
//!
//! Deliberately. A test that expected a processor count, a cache level or a
//! verdict would fail on the next runner shape rather than on a defect, and
//! would have to be loosened until it asserted nothing. What it checks is that
//! whatever this host produced, the report's parts agree **with each other** --
//! a property every host must satisfy, including one whose topology cannot be
//! read at all.

use windows_placement_probe::fingerprint::Fingerprint;
use windows_platform_probes::report_oracle;
use windows_platform_probes::topology::{invariant, measure};
use windows_platform_probes::topology_report::{attribution, report, report_unmeasured};

/// The report exactly as `probe-topology` composes it.
///
/// Composed here rather than by running the binary and reading its stdout,
/// because the oracle needs the report as a value; the binary is covered
/// separately by the stdout test. The sequence mirrors `src/bin/topology.rs`
/// deliberately -- a report assembled some other way would be checking an
/// artifact no one ships.
fn real_report() -> (String, bool) {
    let before = Fingerprint::discover();
    let measured = measure();
    let after = Fingerprint::discover();
    let banner = attribution(&before, &after);

    match measured {
        Ok(observation) => (report(&banner, &observation), true),
        Err(error) => (report_unmeasured(&banner, &error), false),
    }
}

#[test]
fn a_report_rendered_from_this_host_is_well_formed() {
    // **The real host, which no fixture can stand in for.** A fixture is a
    // report somebody wrote down, so it can only exercise shapes its author
    // imagined -- and the defect this file was built around was a state nobody
    // had. On CI this runs across the hosted runner fleet, a slow survey of
    // shapes no fixture anticipates.
    //
    // It asserts nothing about this MACHINE, deliberately: a test expecting a
    // processor count or a verdict would fail on the next runner shape rather
    // than on a defect. What it checks is that whatever this host produced, the
    // row a survey will mine is well-formed -- a property every host satisfies,
    // including one whose topology cannot be read at all.
    let (text, measured) = real_report();

    report_oracle::assert_corresponds(&text);
    assert!(
        report_oracle::row(&text).is_some(),
        "every report carries exactly one row, including an unmeasured one -- \
         that is what lets a survey tell a host where discovery FAILED from a \
         job that never ran the probe (measured: {measured})\n\n{text}"
    );
}

// --- M2.12: the shape corpus ------------------------------------------------

use windows_platform_probes::topology::{BracketOutcome, CacheLevel, CoreShape, Observation};

/// A host shape, named by the renderer branch it exists to reach.
struct Shape {
    what: &'static str,
    observation: Observation,
}

/// The banner a run of this shape would carry.
///
/// **Derived from the observation, not a constant.** A fixed banner made the
/// corpus's own multi-group shape contradict its body -- 128 processors under a
/// `4p` banner -- and the oracle reported it correctly on the first run. That is
/// the banner rule working, but it is the CORPUS that was wrong: a real run
/// reads both from the same machine, so a shape that varies the processor count
/// has to vary the banner with it or it is not a shape the probe could produce.
fn banner_for(observation: &Observation) -> String {
    // **The architecture comes from the build, not from a literal.** The
    // renderer publishes `std::env::consts::ARCH`, so a banner hard-coding
    // `x86_64` agrees with the body on an x86_64 host and contradicts it
    // everywhere else. Measured: on `i686-pc-windows-msvc` both corpus tests
    // failed with `prose: "x86_64"` against `ndjson: "x86"`. CI builds
    // `aarch64` but does not run the suite there, so nothing caught it -- the
    // shape blindness this corpus exists for, in the one dimension the corpus
    // itself could not vary.
    format!(
        "host:  {} {}p/{}c",
        std::env::consts::ARCH,
        observation.online_processors,
        observation.cores.len()
    )
}

/// The observation every shape starts from: agreeing, single level, nothing in
/// doubt. Each shape below changes only what it is named for.
fn base() -> Observation {
    Observation {
        online_processors: 4,
        groups: 1,
        numa_domains: 1,
        numa_domains_without_processors: 0,
        numa_domains_only_in_cpu_sets: 0,
        numa_domains_unreported: 0,
        numa_domains_with_conflicting_labels: 0,
        topology_was_measured: true,
        cores_only_in_cpu_sets: 0,
        cores_without_processors: 0,
        packages_without_processors: 0,
        overlapping_walk_relations: 0,
        described_relations: 0,
        unreported_relations: 0,
        processor_attribute_conflicts: 0,
        highest_numa_node: Some(0),
        packages: 1,
        cores: vec![CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 0,
            processors: 4,
        }],
        caches: vec![CacheLevel {
            level: 1,
            processors_per_domain: vec![4],
        }],
        partitioning_cache_level: Some(1),
        enumeration_anomalies: Vec::new(),
        coherence: windows_topology_sys::Coherence::Agreed,
        raw_active_processors: 4,
        raw_group_count: 1,
        raw_highest_numa_node: Some(0),
        bracket: BracketOutcome::HeldStill,
    }
}

/// Representative shapes this renderer can be driven into, by the branch each
/// reaches.
///
/// **Not every shape, and the difference is recorded rather than glossed.** This
/// said "every shape this renderer can be driven into", which describes a closed
/// set. Each entry below varies ONE dimension, so a branch selected by two at
/// once is unreachable here by construction -- the reason the crossed
/// anomaly-and-disagreement case at the end had to be written by hand, and the
/// reason M2.17 exists. Until the cross product is generated, read this as a
/// sample chosen from the renderer's branches, not as a proof of coverage.
///
/// **Which combinations are legal is DERIVED from `cross_check`, not assumed.**
/// It pushes a `parse_incomplete` entry for an empty cache survey, for a named
/// level with no summary, for a cpu-sets-only NUMA domain and for an unmeasured
/// topology -- and a non-empty `parse_incomplete` forces the verdict away from
/// `agree`. So `NoLevelsReported` and `SummaryMissing` CANNOT be agreeing
/// shapes, and constructing them as such would be building a report the crate
/// cannot produce.
fn shapes() -> Vec<Shape> {
    let mut shapes = Vec::new();
    let mut push = |what, observation| shapes.push(Shape { what, observation });

    push("the level arm, agreeing", base());

    let mut disagreeing = base();
    disagreeing.raw_active_processors = 8;
    push("a counter that disagrees with the enumeration", disagreeing);

    let mut disagreeing_groups = base();
    disagreeing_groups.raw_group_count = 3;
    push("a group counter that disagrees", disagreeing_groups);

    let mut incomplete = base();
    incomplete.numa_domains_only_in_cpu_sets = 2;
    push("a parse in doubt, with the verdict incomplete", incomplete);

    let mut not_unique = base();
    not_unique.partitioning_cache_level = None;
    not_unique.caches = vec![CacheLevel {
        level: 2,
        processors_per_domain: vec![2, 2],
    }];
    push("no unique outermost partitioning level", not_unique);

    let mut no_partitions = base();
    no_partitions.partitioning_cache_level = None;
    push("no level that partitions the machine", no_partitions);

    let mut no_levels = base();
    no_levels.partitioning_cache_level = None;
    no_levels.caches = Vec::new();
    push("no cache levels reported at all", no_levels);

    let mut summary_missing = base();
    summary_missing.partitioning_cache_level = Some(9);
    push(
        "a named level the survey carries no summary for",
        summary_missing,
    );

    let mut heterogeneous = base();
    heterogeneous.cores = vec![
        CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 0,
            processors: 2,
        },
        CoreShape {
            simultaneous_multithreading: false,
            efficiency_class: 1,
            processors: 2,
        },
    ];
    push("more than one efficiency class", heterogeneous);

    let mut grouped = base();
    grouped.groups = 2;
    grouped.raw_group_count = 2;
    grouped.online_processors = 128;
    grouped.raw_active_processors = 128;
    grouped.cores = vec![CoreShape {
        simultaneous_multithreading: true,
        efficiency_class: 0,
        processors: 128,
    }];
    grouped.caches = vec![CacheLevel {
        level: 1,
        processors_per_domain: vec![64, 64],
    }];
    push("more than one processor group", grouped);

    let mut numa = base();
    numa.numa_domains = 3;
    numa.numa_domains_without_processors = 1;
    numa.highest_numa_node = Some(2);
    numa.raw_highest_numa_node = Some(2);
    push("NUMA domains, one of them with no processors", numa);

    let mut no_numa_counter = base();
    no_numa_counter.highest_numa_node = None;
    no_numa_counter.raw_highest_numa_node = None;
    push("a NUMA counter that could not be read", no_numa_counter);

    let mut deep = base();
    deep.partitioning_cache_level = Some(3);
    deep.caches = vec![
        CacheLevel {
            level: 1,
            processors_per_domain: vec![1, 1, 1, 1],
        },
        CacheLevel {
            level: 2,
            processors_per_domain: vec![2, 2],
        },
        CacheLevel {
            level: 3,
            processors_per_domain: vec![4],
        },
    ];
    push("several cache levels", deep);

    let mut unmeasured_topology = base();
    unmeasured_topology.topology_was_measured = false;
    push(
        "a topology not measured from a running machine",
        unmeasured_topology,
    );

    // **The states an earlier version of this corpus never reached.** Every
    // shape above leaves `bracket`, `coherence` and `enumeration_anomalies` at
    // their agreeing defaults, so three `cross_check` branches went unrendered
    // -- including the diagnostic sentence the oracle parses the ANOMALY COUNT
    // out of, which meant the corpus exercised no report where that rule could
    // fire. It claimed to drive the renderer through its branches while leaving
    // those untouched. Found by a review.
    let mut changed = base();
    changed.bracket = BracketOutcome::Changed;
    push("a machine that changed under the run", changed);

    let mut not_established = base();
    not_established.bracket = BracketOutcome::NotEstablished;
    push("a bracket left open at one end", not_established);

    let mut disagreed = base();
    disagreed.coherence = windows_topology_sys::Coherence::Disagreed {
        attempts: 3,
        walk_only: Vec::new(),
        cpu_sets_only: Vec::new(),
    };
    push("two sources that described different processors", disagreed);

    let mut anomalies = base();
    anomalies.enumeration_anomalies = vec![
        windows_topology_sys::EnumerationAnomaly {
            source: windows_topology_sys::Source::RelationshipWalk,
            offset: 0,
            kind: windows_topology_sys::AnomalyKind::TrailingBytes { remaining: 8 },
        },
        windows_topology_sys::EnumerationAnomaly {
            source: windows_topology_sys::Source::CpuSets,
            offset: 16,
            kind: windows_topology_sys::AnomalyKind::TruncatedArray {
                declared: 4,
                decoded: 2,
            },
        },
    ];
    push("records that failed to decode", anomalies);

    // **The tag on a diagnostic entry depends on the verdict, so the shapes
    // have to be crossed rather than listed.** `CrossCheck` writes its
    // `parse_incomplete` entries as `- {caveat}` under `INCOMPLETE` but as
    // `(parse incomplete) {caveat}` under `DISAGREE`, where a reader needs to
    // tell the disagreement from what was merely not established. Every shape
    // above varies ONE thing, so anomalies only ever appeared under the first
    // spelling and the second was never rendered at all.
    //
    // That is how the anomaly-count rule came to be unread on a disagreeing
    // report while a comment above it said it was read for every verdict:
    // corrupting the count there raised no violation, and no corpus report
    // could show it. Found by a review. This shape is the cross, and it is the
    // only one here that varies two dimensions at once.
    let mut anomalies_while_disagreeing = base();
    anomalies_while_disagreeing.raw_active_processors = 8;
    anomalies_while_disagreeing.enumeration_anomalies =
        vec![windows_topology_sys::EnumerationAnomaly {
            source: windows_topology_sys::Source::RelationshipWalk,
            offset: 0,
            kind: windows_topology_sys::AnomalyKind::TrailingBytes { remaining: 8 },
        }];
    push(
        "records that failed to decode, beside a counter that disagrees",
        anomalies_while_disagreeing,
    );

    // **Several conditions in ONE list, which nothing else here reaches.** Every
    // other shape varies one dimension, so each lands at most one entry per
    // list -- and a one-element list has no order to get wrong. That made the
    // ordering half of `the_row_lists_exactly_the_conditions_the_cross_check_found`
    // vacuous: reversing the row's `parse_incomplete` reddened nothing at all.
    //
    // Found by the guard in `the_corpus_reaches_states_that_block_agreement`,
    // which is there precisely because a corpus cannot report the shape it does
    // not reach.
    let mut several = base();
    several.caches = Vec::new();
    several.cores_only_in_cpu_sets = 1;
    several.numa_domains_unreported = 2;
    several.processor_attribute_conflicts = 1;
    push("several conditions at once, in one list", several);

    shapes
}

#[test]
fn every_representative_shape_agrees_with_itself() {
    // **M2.12.** Every instrument in this file was validated against ONE
    // artifact: this machine's. This host renders exactly one shape -- measured,
    // `agree`, the `Level` arm, non-empty caches, one efficiency class, zero
    // anomalies, x86_64 -- and the defects found by review after review lived in
    // the COMPLEMENT of it, each found only because a person imagined a shape by
    // hand.
    //
    // The corpus is the answer, and it is M2.10's move one level up: M2.10
    // derives the set of FACTS from the artifact, this drives the renderer
    // through its own BRANCHES and checks the artifact each one produces.
    //
    // Rendering is itself the oracle check, because `report` asserts on its own
    // output under this build -- so a shape that contradicts itself panics here
    // rather than needing an assertion of its own.
    for shape in shapes() {
        let text = windows_platform_probes::topology_report::report(
            &banner_for(&shape.observation),
            &shape.observation,
        );

        assert!(!text.is_empty(), "{} rendered nothing at all", shape.what);
    }
}

// --- M3.5: the accounting, re-aimed from the prose at the row ----------------

/// The row's lists that render one prose entry each.
///
/// `enumeration_anomalies` is deliberately absent: it is per-anomaly detail the
/// prose summarises into ONE entry, so it counts on a different axis and is
/// checked against the observation instead.
const DIAGNOSTIC_LISTS: &[&str] = &["disagreements", "not_compared", "parse_incomplete"];

/// Every condition code the row publishes under `keys`.
///
/// **Which keys is a parameter, because the four lists do not all relate to
/// the prose the same way.** The three DIAGNOSTIC lists render one prose entry
/// each. `enumeration_anomalies` does not: the prose folds every anomaly into a
/// single `windows-topology-sys recorded N enumeration anomal...` sentence, so
/// a host with three anomalies publishes three codes beside one prose line.
/// Counting them together made that shape look like a dropped entry.
///
/// Reads the RENDERED row rather than the `CrossCheck` behind it, because what a
/// survey receives is the subject: an assertion against the struct would hold
/// even if the writer published nothing at all.
fn published_codes(text: &str, keys: &[&str]) -> Vec<String> {
    let Some(row) = report_oracle::row(text) else {
        panic!("no single well-formed row in:\n{text}");
    };

    keys.iter()
        .flat_map(|key| report_oracle::list_codes(row, key))
        .collect()
}
/// Whether `text`'s row publishes a condition for an observation in a blocking
/// state.
///
/// **Extracted so the sabotage can invoke the rule rather than restate it.**
/// `a_state_the_row_does_not_publish_fails_the_accounting` used to strip the
/// row's conditions and then assert only that the stripping had worked -- so it
/// demonstrated the sabotage, never that the accounting REJECTS it. The
/// accounting could have been deleted and that test would have stayed green.
/// Found by a review.
///
/// **Per state, not "at least one".** This asked only whether the row published
/// SOME code, which an observation in five blocking states satisfies by
/// publishing one -- so dropping four states' conditions left the rule that
/// names itself `every_state_...` perfectly happy. Measured: truncating the
/// row's `parse_incomplete` to its first entry is caught by three tests that
/// compare the row against `cross_check`, and by this one not at all. That
/// matters because this is the only instrument running the other enumeration --
/// from `invariant`'s states INTO the row -- so its weakness was invisible to
/// everything else. Found by a review of the pull request.
fn publication_holds(observation: &Observation, text: &str) -> bool {
    let published = published_codes(text, DIAGNOSTIC_LISTS);
    invariant::blocking_states(observation)
        .into_iter()
        .all(|state| {
            codes_for(state)
                .iter()
                .any(|code| published.iter().any(|found| found == code))
        })
}

/// The row code(s) that answer `state`.
///
/// **A schema, written down, and exhaustive so it cannot fall behind.** This is
/// the correspondence the milestone exists to enforce -- a state the invariants
/// know about must reach a survey -- and it is not derivable from either side:
/// `blocking_states` computes states from an observation and the renderer emits
/// codes, with nothing in between that already knows the pairing. Writing it
/// here is what makes the check possible; a new state that names no code fails
/// to compile.
///
/// `BracketNotHeld` answers to either code because `cross_check` files a
/// different one depending on how the bracket failed, and both are honest
/// reports of the same blocking state.
fn codes_for(state: invariant::BlockingState) -> &'static [&'static str] {
    use invariant::BlockingState as State;
    match state {
        State::PartitioningSummaryMissing => &["partitioning_summary_missing"],
        State::EnumerationAnomalies => &["enumeration_anomalies"],
        State::NotMeasured => &["not_measured"],
        State::NoCacheLevels => &["no_cache_levels"],
        State::NoPackages => &["no_packages"],
        State::NoCores => &["no_cores"],
        State::ContradictoryCore => &["contradictory_cores"],
        State::UnnumberedCacheLevel => &["unnumbered_cache_levels"],
        State::EnumerationsDisagreed => &["enumerations_disagreed"],
        State::BracketNotHeld => &["machine_changed", "bracket_not_established"],
    }
}

#[test]
fn every_state_that_blocks_agreement_reaches_the_row() {
    // **This is the rule M3.1 established, given an instrument at last.** A
    // renderer may not tell a reader something the row cannot tell a survey --
    // and nothing enforced that, because every instrument in this crate
    // enumerated the ROW's keys and so could only ask "does anything read this
    // key?", never "does the prose state a fact the row omits?".
    //
    // Measured, which is how the gap was found rather than reasoned:
    // `CrossCheck::disagreements` reached the prose as one line per
    // disagreement and reached the row as nothing at all, so a survey saw
    // `"cross_check":"disagree"` without learning WHICH counter disagreed. It
    // survived 41 review rounds and a zero-survivor mutation sweep because all
    // of them start from what the row publishes.
    //
    // The enumeration here runs the other way: for every state
    // `topology::invariant` knows forbids an agreeing verdict, render a report
    // in that state and require the row to carry a code for it.
    for shape in shapes() {
        let text = windows_platform_probes::topology_report::report(
            &banner_for(&shape.observation),
            &shape.observation,
        );

        let blocking = invariant::blocking_states(&shape.observation);
        if blocking.is_empty() {
            continue;
        }

        assert!(
            publication_holds(&shape.observation, &text),
            "{}: the observation is in {} state(s) that forbid agreement -- \
             {blocking:?} -- and the row publishes no condition at all. A \
             survey reading it would see a verdict it cannot account \
             for.\n\n--- the report ---\n{text}",
            shape.what,
            blocking.len(),
        );
    }
}

#[test]
fn the_corpus_reaches_an_observation_in_several_blocking_states_at_once() {
    // **The rule above is per-state, and a corpus of single-state shapes cannot
    // tell that apart from "at least one".** Its previous form was satisfied by
    // any one published code, and no shape with two states would have exposed
    // that -- which is why the reachability is asserted rather than assumed: a
    // corpus cannot report the shape it never reaches.
    let deepest = shapes()
        .into_iter()
        .map(|shape| {
            (
                shape.what,
                invariant::blocking_states(&shape.observation).len(),
            )
        })
        .max_by_key(|(_, states)| *states)
        .expect("the corpus is not empty");

    assert!(
        deepest.1 >= 2,
        "the deepest shape in the corpus is `{}` with {} blocking state(s), so \
         the per-state rule is never asked to distinguish one state from \
         several",
        deepest.0,
        deepest.1,
    );
}

#[test]
fn the_row_lists_exactly_the_conditions_the_cross_check_found() {
    // **The rule that replaced a prose count, and the last prose parsing in the
    // matrix went with it.** This compared the row's condition count against a
    // count of prose lines -- which meant filtering rendered text by line prefix
    // and turning it into a number, the one remaining place the test matrix
    // obtained structured data by reading prose.
    //
    // What replaces it is strictly stronger and never reads a sentence: the
    // row's codes must EQUAL the cross-check's codes, in order. A count could
    // only catch a dropped entry; this catches a dropped one, a reordered one,
    // and a substituted one.
    //
    // Be clear about what it is: the row is BUILT from these lists, so this is
    // the writer being checked against its input, not an independent reading.
    // That is exactly the check worth having here -- the writer is the one thing
    // no amount of typing upstream can check for itself -- but it is narrower
    // than "the report is correct" and should not be read as that.
    for shape in shapes() {
        let text = windows_platform_probes::topology_report::report(
            &banner_for(&shape.observation),
            &shape.observation,
        );

        let check = shape.observation.cross_check();
        let expected: Vec<String> = check
            .disagreements
            .iter()
            .map(|entry| entry.code().to_owned())
            .chain(
                check
                    .not_compared
                    .iter()
                    .map(|entry| entry.code().to_owned()),
            )
            .chain(
                check
                    .parse_incomplete
                    .iter()
                    .map(|entry| entry.code().to_owned()),
            )
            .collect();

        assert_eq!(
            published_codes(&text, DIAGNOSTIC_LISTS),
            expected,
            "{}: the row must publish every condition the cross-check found, in \
             order\n\n--- the report ---\n{text}",
            shape.what,
        );
    }
}
#[test]
fn the_corpus_reaches_states_that_block_agreement() {
    // **The guard that stops the two tests above passing for nothing.** Both
    // skip or trivially satisfy a shape in no blocking state, so a corpus that
    // had drifted to all-healthy shapes would leave them green while checking
    // nothing -- the failure mode this crate keeps meeting.
    //
    // Stated as a relation rather than a count: at least one shape blocks, and
    // at least one does not, so both sides of every rule above are exercised.
    let blocking = shapes()
        .iter()
        .filter(|shape| !invariant::blocking_states(&shape.observation).is_empty())
        .count();

    assert!(
        blocking > 0,
        "no shape in the corpus is in a blocking state, so the publication \
         rules are vacuous"
    );
    assert!(
        blocking < shapes().len(),
        "every shape blocks, so the acceptance half of the publication rules \
         is never exercised"
    );

    // **ORDER is only a claim where there is more than one entry to order.**
    // `the_row_lists_exactly_the_conditions_the_cross_check_found` compares the
    // row's codes against the cross-check's as a SEQUENCE, which is what makes
    // it stronger than the prose count it replaced -- but a corpus whose shapes
    // each carry at most one condition can never tell a sequence from a set.
    //
    // Measured, and this is why the guard exists: reversing the row's
    // `parse_incomplete` order reddened nothing until a multi-condition shape
    // was in the corpus.
    // Within ONE list, not summed across the three. Summing was the first
    // version of this guard and it passed while the sabotage still reddened
    // nothing: a shape carrying one `not_compared` and one `parse_incomplete`
    // has two conditions and no order to get wrong, because reversing a
    // one-element list is the identity.
    let most = shapes()
        .iter()
        .map(|shape| {
            let check = shape.observation.cross_check();
            check
                .disagreements
                .len()
                .max(check.not_compared.len())
                .max(check.parse_incomplete.len())
        })
        .max()
        .unwrap_or_default();

    assert!(
        most >= 2,
        "no shape carries two conditions in ONE list, so the ordering half of \
         the publication rule is vacuous -- it cannot tell a sequence from a set"
    );
}

#[test]
fn a_state_the_row_does_not_publish_fails_the_accounting() {
    // **The accounting's own sabotage, so it cannot go quietly blind.** A rule
    // that enumerates states and finds them all published is indistinguishable
    // from one that enumerates nothing -- unless something shows it failing.
    //
    // Reproduces the `disagreements` defect exactly: a report in a blocking
    // state whose row carries no condition for it.
    let mut observation = base();
    observation.partitioning_cache_level = Some(9);

    let text =
        windows_platform_probes::topology_report::report(&banner_for(&observation), &observation);

    assert!(
        !invariant::blocking_states(&observation).is_empty(),
        "the fixture must be in a blocking state or this shows nothing"
    );
    assert!(
        !published_codes(&text, DIAGNOSTIC_LISTS).is_empty(),
        "the row publishes it today, which is what the rule requires"
    );

    // Now empty every diagnostic list in the row, which is what a renderer that
    // forgot to publish one would produce.
    //
    // **Done by parsing, emptying and re-rendering, not by cutting the text.**
    // Two earlier versions cut it: the first split on commas, which sliced the
    // entries in half once they became objects; the second used a span helper in
    // the oracle that was not string-aware. Both are the same mistake -- a
    // sabotage that hand-parses is a sabotage that can stop sabotaging while
    // still passing, and it leaves the rule it guards unguarded. Rebuilding from
    // a parse also produces a row that is genuinely valid, so what this feeds the
    // accounting is a report a renderer could really have emitted.
    let stripped = text
        .lines()
        .map(|line| {
            if !line.starts_with('{') {
                return line.to_owned();
            }
            let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(line) else {
                return line.to_owned();
            };
            for key in DIAGNOSTIC_LISTS {
                if let Some(list) = parsed.get_mut(*key)
                    && list.is_array()
                {
                    *list = serde_json::Value::Array(Vec::new());
                }
            }
            parsed.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        published_codes(&stripped, DIAGNOSTIC_LISTS).is_empty(),
        "the sabotage must actually remove the conditions: {stripped}"
    );

    // **And the accounting must REJECT it.** Asserting only that the stripping
    // worked demonstrated the sabotage and nothing else -- the rule could have
    // been deleted and this stayed green, which is the shape of vacuity this
    // whole suite exists to avoid. Calling the same predicate the corpus rule
    // calls is what makes this a test of the rule.
    assert!(
        !publication_holds(&observation, &stripped),
        "a report in a blocking state whose row publishes nothing must fail the \
         accounting: {stripped}"
    );
    assert!(
        publication_holds(&observation, &text),
        "and the unsabotaged report must pass it, or the rule rejects \
         everything: {text}"
    );
}

#[test]
fn the_row_publishes_one_code_per_anomaly_the_observation_carries() {
    // **The axis `DIAGNOSTIC_LISTS` deliberately leaves out.** The prose folds
    // every anomaly into one sentence, so the prose cannot say how many there
    // were beyond the number inside that sentence -- and checking a number
    // inside a sentence is the prose-reading this milestone retired.
    //
    // Checked against the OBSERVATION instead, which is the artifact the row is
    // supposed to be faithful to. That is the relation worth having: a survey
    // grouping anomalies by kind is reading this list, and it must have one
    // entry per anomaly the enumeration actually recorded.
    for shape in shapes() {
        let text = windows_platform_probes::topology_report::report(
            &banner_for(&shape.observation),
            &shape.observation,
        );

        assert_eq!(
            published_codes(&text, &["enumeration_anomalies"]).len(),
            shape.observation.enumeration_anomalies.len(),
            "{}: the observation carries {} anomal(ies) and the row publishes \
             {:?}\n\n--- the report ---\n{text}",
            shape.what,
            shape.observation.enumeration_anomalies.len(),
            published_codes(&text, &["enumeration_anomalies"]),
        );
    }
}

#[test]
fn the_corpus_reaches_a_shape_that_records_anomalies() {
    // The guard for the rule above: on an all-clean corpus it compares zero
    // against zero on every shape and establishes nothing.
    assert!(
        shapes()
            .iter()
            .any(|shape| !shape.observation.enumeration_anomalies.is_empty()),
        "no shape records an anomaly, so the per-anomaly rule is vacuous"
    );
}
