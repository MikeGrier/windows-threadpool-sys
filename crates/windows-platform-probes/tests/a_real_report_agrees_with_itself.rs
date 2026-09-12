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
use windows_platform_probes::topology::measure;
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
fn a_report_rendered_from_this_host_agrees_with_itself() {
    let (text, _) = real_report();

    // The whole assertion. Not "the report says X" -- "the report does not
    // contradict itself", which is checkable without knowing anything about
    // the machine.
    report_oracle::assert_corresponds(&text);
}

#[test]
fn the_oracle_is_actually_reading_this_host_s_report() {
    // Without this, the test above is worth nothing on a host whose report the
    // oracle cannot parse: every lookup returns `None`, every comparison is
    // skipped, and it passes having checked exactly zero correspondences.
    //
    // The unit tests pin the oracle against a fixture, which cannot notice the
    // renderer drifting away from it. Only the real artifact can, and only if
    // something requires a violation to appear.
    //
    // So a fact this host really rendered is corrupted, and the oracle must
    // report it. `"processors":0` is the corruption because no host has zero
    // online processors, so it disagrees with the prose on every machine
    // without needing to know what the prose says.
    let (text, measured) = real_report();

    if !measured {
        // `report_unmeasured` carries no counts to corrupt. A host whose
        // topology cannot be read is a legitimate outcome -- "cannot measure"
        // is a third answer in this crate -- and skipping is honest here in a
        // way it would not be for the assertion above, which still ran.
        note("topology could not be read on this host; corruption check skipped");
        return;
    }

    // Every double-rendered fact, not just one. Corrupting a single field would
    // leave the other pairs unguarded: the renderer could drift away from the
    // oracle's other prose labels and this would still pass on the strength of
    // the one.
    //
    // **Each corruption is chosen against the value actually rendered, and keys
    // this host did not render are skipped.** An earlier version replaced every
    // count with `0`, on the reasoning that no host has zero processors, groups,
    // packages or cores -- which is true of those four and false of the report
    // as a whole, and the difference would have failed on legitimate hosts
    // rather than on a defect:
    //
    //   * `outermost_partitioning_cache_level` is rendered `null` for the three
    //     absent-partitioning variants, and no prose level accompanies it, so
    //     rewriting it to `0` yields no correspondence to violate.
    //   * `"domains":` does not appear at all on a host that reports no caches.
    //   * A successfully measured but incomplete parse can render `0` decoded
    //     packages or cores, where replacing `0` with `0` changes nothing and
    //     the sabotage silently fails to apply.
    //
    // So the replacement is `1` where the report says `0` and `0` otherwise,
    // which is guaranteed to differ from whatever this host rendered, and a key
    // that is absent or `null` is skipped with a reason rather than asserted
    // against.
    //
    // The skip cannot swallow the whole test, and the guard names WHICH facts it
    // requires rather than how many.
    //
    // **A count was not enough, and the difference is not academic.** An earlier
    // version asserted `exercised >= 4` against a running total, which the four
    // CONDITIONAL entries can satisfy on their own -- so if the four
    // unconditional keys drifted out of the renderer, all four would skip, the
    // conditional four would make the total, and this test would pass having
    // checked none of the facts its comment claimed it required. Demonstrated by
    // a reviewer: forcing the four unconditional lookups to miss produced four
    // skip messages and a PASS. The comment claimed coverage while the code
    // enforced quantity, which is the defect this whole branch is about, in the
    // guard written to prevent it.
    let mut exercised: Vec<&str> = Vec::new();

    // `true` where a measured report must carry the fact, so a skip is a defect
    // rather than a legal shape.
    //
    // **Six are unconditional, not four.** An earlier version marked
    // `"numa_domains":` and the `"single":` policy optional on the stated
    // grounds that "the policy and NUMA entries depend on what the host has".
    // They do not, and the renderer says so plainly: `report()` writes the
    // `NUMA domains` prose line and the `"numa_domains"` field with no
    // condition around either, and `domain_counts()` begins every policy table
    // with `("single", 1)` -- a count clamped to one because there is always at
    // least one domain. The claim was about the host; the truth was about the
    // renderer, which is the same confusion in miniature that this whole test
    // exists to catch.
    //
    // Marking a mandatory fact optional inverts the guard: measured, renaming
    // the NDJSON `"numa_domains"` key, and separately renaming the `single`
    // policy, each left this test GREEN -- so it passed precisely when the
    // renderer-to-oracle binding for a mandatory fact disappeared, which is the
    // one event it is here to detect.
    //
    // The two that remain optional are genuinely conditional: the outermost
    // level is `null` on three partitioning variants, and `"domains":` is
    // absent when no caches are reported.
    // The fact each key must be reported under. A LIST, not one name, because
    // one key can be read by different rules depending on the shape the host
    // produced -- see the outermost level below. Every entry is still a specific
    // fact, so a neighbouring rule cannot stand in for any of them.
    for (key, facts, required) in [
        ("\"processors\":", &["online processors"][..], true),
        ("\"groups\":", &["processor groups"][..], true),
        ("\"packages\":", &["packages"][..], true),
        ("\"cores\":", &["physical cores"][..], true),
        ("\"numa_domains\":", &["NUMA domains"][..], true),
        (
            // **Two facts, because two arms name a level.** The `Level` arm
            // renders `outermost cache that partitions the processors it
            // covers: L2`, and `SummaryMissing` renders `BUG IN THIS PROBE: the
            // topology crate named L2 as the outermost` -- different sentences,
            // read by different rules, reported under different fact names. The
            // arms come from one `match` and so are mutually exclusive, which is
            // what makes accepting either of them precise rather than loose.
            //
            // Found by a review. With only the first name here, a host that
            // produced the `SummaryMissing` shape failed this guard even though
            // the field WAS read -- an instrument reporting a defect in the
            // renderer that was really a defect in the instrument.
            "\"outermost_partitioning_cache_level\":",
            &[
                "outermost partitioning cache level",
                "summary-missing outermost level",
            ][..],
            false,
        ),
        // Nested, and reached by their inner keys: `policies` is an object and
        // `caches` an array of objects, so these corrupt the first entry of
        // each rather than the container.
        ("\"single\":", &["policy domain count"][..], true),
        ("\"domains\":", &["cache domain count"][..], false),
    ] {
        let Some(rendered) = rendered_value(&text, key) else {
            assert!(
                !required,
                "{key} is rendered by every measured report, and this one does \
                 not carry it. Either the renderer dropped the field or the key \
                 spelling drifted -- both of which make the correspondence stop \
                 being checked.\n\n--- the report ---\n{text}"
            );
            note(&format!("{key} is not rendered on this host; skipped"));
            continue;
        };
        if rendered == "null" {
            assert!(
                !required,
                "{key} is rendered null, which no measured report does for this \
                 fact.\n\n--- the report ---\n{text}"
            );
            note(&format!(
                "{key} is rendered null on this host, so no prose accompanies it; skipped"
            ));
            continue;
        }

        let corrupted = corrupt_count(&text, key, if rendered == "0" { "1" } else { "0" });
        assert_ne!(
            corrupted, text,
            "corrupting {key} changed nothing, so this proves nothing -- a \
             sabotage that fails to apply is indistinguishable from an \
             instrument that fails to fire.\n\n--- the report ---\n{text}"
        );

        // **A violation naming THIS FACT, not merely one of the right kind.**
        // This guard has now been wrong twice in the same direction, each fix
        // stopping one step short. It first asserted only that the list was
        // non-empty. That was strengthened to require a
        // `ProseAndNdjsonDisagree`, with a comment correctly explaining that
        // corrupting `processors` ALSO trips the cross-check counter rule --
        // and then not acting on it, because the counter rule emits that very
        // variant. So the strengthened form still passed while the prose reader
        // was blind.
        //
        // Measured, one entry blinded at a time by breaking its `DOUBLE_RENDERED`
        // prose label: with the variant-only assertion, `processors` and `groups`
        // both stayed GREEN (masked by their counter rules at
        // `check_counters_against_enumeration`), while `packages` went red
        // because nothing else reads it. Two of the four required facts were
        // unchecked by the guard whose whole purpose is to prove they are
        // checked.
        //
        // Matching on `fact` is what closes it: no neighbouring rule can supply
        // another rule's fact name.
        let violations = report_oracle::check(&corrupted);
        assert!(
            violations.iter().any(|violation| matches!(
                violation,
                report_oracle::Correspondence::ProseAndNdjsonDisagree { fact: named, .. }
                    if facts.contains(named)
            )),
            "the oracle read no prose-against-NDJSON correspondence for {key} \
             (expected one of the facts {facts:?}) in a report this host actually \
             produced, so `a_report_rendered_from_this_host_agrees_with_itself` \
             is passing without checking that fact. The renderer has probably \
             drifted from the prose label the oracle looks for.\n\ngot \
             {violations:#?}\n\n--- the report ---\n{text}"
        );
        exercised.push(key);
    }

    // Named, not counted. Each unconditional fact must appear in what was
    // actually exercised, so no number of conditional entries can stand in for
    // one of them.
    //
    // This list is deliberately a SECOND statement of which facts are
    // mandatory, rather than being derived from the `required` column above.
    // Deriving it would make deleting a table row silently legal -- the row
    // would vanish from both the loop and the requirement in one edit. Two
    // independent statements mean a row cannot be dropped without this list
    // noticing, which is the same reasoning the oracle itself is built on:
    // relate two renderings rather than trusting one.
    for required in [
        "\"processors\":",
        "\"groups\":",
        "\"packages\":",
        "\"cores\":",
        "\"numa_domains\":",
        "\"single\":",
    ] {
        assert!(
            exercised.contains(&required),
            "{required} was never exercised on this host, so the correspondence \
             it names went unchecked. Exercised: {exercised:?}\n\n\
             --- the report ---\n{text}"
        );
    }
}

/// The value the NDJSON renders for `key`, or `None` when it renders none.
///
/// Used to decide whether a fact is present at all before corrupting it, and to
/// choose a replacement that differs from what this host actually rendered.
fn rendered_value(report: &str, key: &str) -> Option<String> {
    let line = report.lines().find(|line| line.starts_with('{'))?;
    let start = line.find(key)? + key.len();
    let end = line[start..]
        .find([',', '}'])
        .map_or(line.len(), |offset| start + offset);

    Some(line[start..end].to_owned())
}

/// Rewrite one NDJSON count to `value`, which the caller picks to differ from
/// what this host rendered.
fn corrupt_count(report: &str, key: &str, value: &str) -> String {
    report
        .lines()
        .map(|line| {
            if line.starts_with('{') {
                replace_json_number(line, key, value)
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replace the numeric value following `key` in a flat JSON line.
fn replace_json_number(line: &str, key: &str, value: &str) -> String {
    let Some(start) = line.find(key) else {
        return line.to_owned();
    };
    let after = start + key.len();
    let end = line[after..]
        .find([',', '}'])
        .map_or(line.len(), |offset| after + offset);

    format!("{}{key}{value}{}", &line[..start], &line[end..])
}

// --- M2.10: the fact set, derived from the artifact rather than restated ------

/// Why a fact may legitimately go unread on some host.
#[derive(Clone, Copy)]
enum Silence {
    /// Never. The prose always states this fact, so it must always be read.
    Never,
    /// When the key renders this exact value, the prose makes no matching claim.
    AtValue(&'static str),
    /// When the key renders this value AND the verdict is not `agree`.
    ///
    /// **Two different rules read these counts, and only one of them is always
    /// available.** At zero the prose sentence carrying the count is absent, so
    /// the count comparison cannot fire -- but under an AGREEING verdict a
    /// nonzero value is a violation in its own right, so the zero-to-nonzero
    /// corruption is still caught. Declaring the key silent at zero outright
    /// excused both, which made the assertion vacuous on this host and let the
    /// declaration's own prose claim a guarantee it had just given away.
    /// Measured: with no excuse at all, exactly one mutation went unnoticed --
    /// the deletion -- and on a DISAGREE shape, exactly one went unnoticed --
    /// the corruption. Neither blanket answer is right; the verdict is what
    /// separates them.
    AtValueUnlessAgreeing(&'static str),
    /// When no `host:` line names an architecture. The banner is the only prose
    /// rendering of the architecture, and a failed discovery renders `UNKNOWN`,
    /// which names none -- so on a host whose bracket reads both failed there is
    /// nothing to relate. Conditioned on the REPORT rather than on the key's own
    /// value, which is why this cannot be expressed as `AtValue`.
    WhenNoBannerArchitecture,
}

/// Every fact a report may publish, what this test requires of it, and the
/// correspondences that count as reading it.
///
/// `reads` is the crux. An earlier version treated a key as read when ANY
/// violation appeared after corrupting it, which is the same defect this branch
/// spent rounds fixing elsewhere: corrupting `parse_incomplete` on a
/// heterogeneous report also trips `UncaveatedClaimUnderDoubt`, so the
/// diagnostics reader could be deleted entirely and this test would stay green.
/// Naming the facts that belong to each key is what makes the measurement mean
/// something. Found by a review.
///
/// `empty_replacement` is the minimal non-empty value to substitute for an empty
/// container. Both empty shapes still have a second rendering -- the renderer
/// emits `efficiency classes: []` unconditionally, and `caches:` followed by
/// `none reported` -- so treating them as silent would let a no-class or
/// no-cache host pass while the reader was gone. Also found by a review.
/// `absent_is_silent_at` is separate from `silence` because the two questions
/// have different answers for the diagnostic counts. At `0` those keys render NO
/// prose entry, so deleting them leaves no claim for the dropped-counterpart
/// rule to answer and the oracle is right to say nothing -- while CORRUPTING
/// them to nonzero is still caught, by the rule that a non-`agree` count
/// contradicts an agreeing verdict. Declaring them `Silence::AtValue("0")`
/// would have excused both and thrown away coverage that exists, so the
/// narrower statement is the true one.
struct Fact {
    key: &'static str,
    silence: Silence,
    reads: &'static [&'static str],
    empty_replacement: Option<&'static str>,
    /// The value at which the prose renders nothing, so the key's ABSENCE has no
    /// prose claim to contradict. Applies only to the deletion mutation.
    absent_is_silent_at: Option<&'static str>,
    /// The prose label of an independently-read counter for this key, if one
    /// exists. Corrupting THAT line moves a side only the counter rule reads,
    /// which is what forces that rule to be exercised separately from the
    /// prose/NDJSON rule that shares the key.
    counter_prose: Option<&'static str>,
    why: &'static str,
}

const fn fact(key: &'static str, reads: &'static [&'static str]) -> Fact {
    Fact {
        key,
        silence: Silence::Never,
        reads,
        empty_replacement: None,
        absent_is_silent_at: None,
        counter_prose: None,
        why: "",
    }
}

/// The first object in the rendered `caches` array, if the report has one.
///
/// Taken from the artifact rather than assumed, so a host with no L1 -- or no
/// caches at all -- is described rather than mismatched.
fn first_cache_object(report: &str) -> Option<String> {
    let line = report.lines().find(|line| line.starts_with('{'))?;
    let at = line.find(r#""caches":[{"#)? + r#""caches":["#.len();
    let rest = &line[at..];
    let end = rest.find('}')? + 1;

    Some(rest[..end].to_owned())
}

/// The report with `label`'s prose value replaced by one that disagrees.
///
/// Moves only the prose side, so the NDJSON still agrees with its own prose and
/// the only rule that can notice is the one reading this counter.
fn corrupt_counter_prose(report: &str, label: &str) -> Option<String> {
    let line = report.lines().find(|line| line.starts_with(label))?;
    let rendered = line[label.len()..].trim();
    let flipped = if rendered == "0" { "1" } else { "0" };

    Some(report.replace(line, &format!("{label}{flipped}")))
}

const FACTS: &[Fact] = &[
    Fact {
        silence: Silence::WhenNoBannerArchitecture,
        why: "a report whose bracket reads both failed renders `UNKNOWN`, which \
              names no architecture, so the body's `arch` has no prose to relate",
        ..fact("arch", &["architecture"])
    },
    // **Two rules read these keys, so the mutation set has to separate them.**
    // Corrupting the NDJSON value moves the enumerated side, which BOTH the
    // prose/NDJSON rule and the cross-check counter rule can see -- so the
    // ordinary rule's violation satisfied the accounting and masked whether the
    // counter rule was read at all. Measured: deleting both counter rules left
    // every test in this file green.
    //
    // `counter_prose` names the line only the counter rule reads. Corrupting it
    // leaves the enumeration agreeing with its own prose, so the only violation
    // available names the counter fact -- and if that rule is gone, nothing is
    // reported and the accounting fails, which is the point.
    Fact {
        counter_prose: Some("  GetActiveProcessorCount     : "),
        ..fact(
            "processors",
            &[
                "online processors",
                "active processor count against the enumeration",
            ],
        )
    },
    Fact {
        counter_prose: Some("  GetActiveProcessorGroupCount: "),
        ..fact(
            "groups",
            &[
                "processor groups",
                "active group count against the enumeration",
            ],
        )
    },
    fact("packages", &["packages"]),
    fact("numa_domains", &["NUMA domains"]),
    fact(
        "numa_domains_without_processors",
        &["NUMA domains without processors"],
    ),
    fact("cores", &["physical cores"]),
    Fact {
        empty_replacement: Some("[0]"),
        ..fact("efficiency_classes", &["efficiency classes"])
    },
    Fact {
        empty_replacement: Some(r#"[{"level":9,"domains":9}]"#),
        ..fact("caches", &["cache levels", "cache domain count"])
    },
    Fact {
        silence: Silence::AtValue("null"),
        why: "three of the five partitioning variants publish null, and their \
              prose names no level",
        ..fact(
            "outermost_partitioning_cache_level",
            &[
                "outermost partitioning cache level",
                "summary-missing outermost level",
            ],
        )
    },
    fact(
        "outermost_partitioning_cache",
        &["outermost partitioning answer"],
    ),
    fact("policies", &["policy domain count", "policy names"]),
    Fact {
        silence: Silence::AtValue("\"not_measured\""),
        why: "`report_unmeasured` renders no verdict line, so the short object's \
              cross_check has no prose to relate",
        ..fact("cross_check", &["cross-check verdict"])
    },
    // **Read at every value, but ABSENT only matters when the prose speaks.**
    // Both reach the prose as entries the renderer emits only when the count is
    // above zero, so at zero there is no line for a dropped counterpart to
    // contradict -- while corrupting either to nonzero is still caught, because
    // a non-zero count beside an agreeing verdict is a violation in its own
    // right. `Silence::AtValue("0")` would have excused both mutations and
    // discarded that second guarantee.
    Fact {
        absent_is_silent_at: Some("0"),
        why: "at zero the renderer emits no `(not compared)` entry, so the \
              absence of the field contradicts nothing",
        ..fact(
            "not_compared",
            &["not compared count", "incomplete-verdict listing count"],
        )
    },
    Fact {
        absent_is_silent_at: Some("0"),
        why: "at zero the renderer emits no `(parse incomplete)` entry, so the \
              absence of the field contradicts nothing",
        ..fact(
            "parse_incomplete",
            &["parse incomplete count", "incomplete-verdict listing count"],
        )
    },
    // **Silent when ABSENT at zero, not silent at zero.** This was declared
    // `Silence::AtValue("0")`, which excuses every mutation on a host rendering
    // no anomalies -- including the zero-to-nonzero corruption that the comment
    // below calls load-bearing. Measured: with the excuse removed, exactly ONE
    // of the two mutations goes unnoticed, and it is the DELETION. The
    // corruption is caught, by the rule that a nonzero count cannot sit beside
    // an agreeing verdict -- so blanket silence threw away a guarantee this host
    // does provide, and let the comment claim one the classification denied.
    //
    // The narrower declaration is the same one `not_compared` and
    // `parse_incomplete` needed, and this key should have been swept with them.
    Fact {
        silence: Silence::AtValueUnlessAgreeing("0"),
        absent_is_silent_at: Some("0"),
        why: "the count reaches the prose only inside the `windows-topology-sys \
              recorded N enumeration anomal...` sentence, which the renderer \
              emits only when there are anomalies. At zero that sentence is \
              absent, so on a DISAGREE or INCOMPLETE host there is no second \
              rendering -- and under `agree` a nonzero value is a violation in \
              its own right, so the fact is never silently wrong. **This \
              declaration was made once, in c75d74e, and then lost when this \
              table was restructured two commits later; nothing caught it \
              because THIS host reports `agree`, where the key is read anyway. \
              Found by a review, twice.**",
        ..fact("enumeration_anomalies", &["enumeration anomaly count"])
    },
    Fact {
        silence: Silence::AtValue("0"),
        why: "the renderer emits the `(N reported only by CPU Sets` line only \
              when the count is above zero, so zero is silence rather than \
              agreement",
        ..fact(
            "numa_domains_only_in_cpu_sets",
            &["NUMA domains reported only by CPU Sets"],
        )
    },
];

/// Keys with no second rendering at all, and why.
const NEVER_COMPARED: &[(&str, &str)] = &[(
    "reason",
    "a routing tag for a mining pass, naming which probe emitted the row. The \
     prose never states it, so there is no correspondence to check",
)];

#[test]
fn every_fact_the_renderer_publishes_is_accounted_for() {
    // **M2.10.** Six unread double-renderings were found by six reviewers and
    // none by this suite, because a rule is added per fact and nothing derived
    // the SET of facts from the renderer.
    //
    // The derivation takes both halves from places that cannot drift: the SET is
    // enumerated from the NDJSON line of a report the renderer really produced,
    // and read-or-unread is MEASURED by corrupting each value and asking the
    // oracle. What is declared is the classification -- which facts belong to a
    // key, and when silence is legitimate -- and every part of that declaration
    // is itself measured.
    account_for_every_fact(&real_report().0, "the measured report");
    account_for_every_fact(&unmeasured_report(), "the unmeasured report");
}

/// The short report, rendered through the real path with a real banner.
fn unmeasured_report() -> String {
    let before = Fingerprint::discover();
    let after = Fingerprint::discover();

    report_unmeasured(
        &attribution(&before, &after),
        &std::io::Error::other("a simulated discovery failure"),
    )
}

/// Require every fact `text` publishes to be classified, and every claim in that
/// classification to hold.
fn account_for_every_fact(text: &str, shape: &str) {
    let keys = ndjson_keys(text);
    assert!(
        !keys.is_empty(),
        "{shape} published no machine-readable facts at all, so the enumeration \
         is broken rather than the renderer.\n\n--- the report ---\n{text}"
    );

    for key in &keys {
        let classified = FACTS.iter().find(|entry| entry.key == key);
        let never = NEVER_COMPARED.iter().find(|(name, _)| name == key);

        assert!(
            classified.is_some() || never.is_some(),
            "{shape} publishes `{key}`, and nothing here says whether the oracle \
             reads it. THIS IS THE POINT OF THIS TEST: a fact was added to the \
             report and no rule was added to relate it to its prose. Either add \
             the rule and list `{key}` in FACTS, or say why it has no second \
             rendering in NEVER_COMPARED.\n\n--- the report ---\n{text}"
        );

        let rendered = raw_value(text, key).unwrap_or_default();

        if let Some(fact) = classified {
            // Every mutation site in turn, not just the first. Corrupting only
            // the first number inside `caches` changes a LEVEL, so the
            // domain-count reader could be deleted and the level-membership rule
            // would still fire and hide it. Found by a review.
            let mut mutations = corruptions(text, fact.key, fact.empty_replacement);

            // The prose-side mutation, where a second rule reads this key from a
            // line of its own. Appended BEFORE the deletion-excuse slice below
            // would trim the tail, so it is judged like any other corruption.
            //
            // **Only where the counter rule can fire.** It reads the counters
            // solely under an agreeing verdict -- on a report that already says
            // its counters disagree, a counter contradiction is the subject
            // rather than a violation. Generating the mutation anyway made the
            // corpus' `a counter that disagrees with the enumeration` shape fail
            // for the rule's correct behaviour, which is the over-constraint
            // this module treats as the same defect as under-specifying.
            if let Some(label) = fact.counter_prose
                && raw_value(text, "cross_check").as_deref() == Some("\"agree\"")
                && let Some(corrupted) = corrupt_counter_prose(text, label)
            {
                let deletion = mutations.pop();
                mutations.push(corrupted);
                mutations.extend(deletion);
            }

            // The deletion is the LAST mutation `corruptions` appends, and it is
            // judged on its own terms: a key whose value renders no prose leaves
            // nothing for a dropped counterpart to contradict, so its absence is
            // legitimately silent even though corrupting it is not.
            let absence_excused = fact
                .absent_is_silent_at
                .is_some_and(|value| rendered == value);
            let judged = if absence_excused {
                &mutations[..mutations.len().saturating_sub(1)]
            } else {
                &mutations[..]
            };

            let unread: Vec<&String> = judged
                .iter()
                .filter(|corrupted| !reads_the_fact(corrupted, fact))
                .collect();

            let excused = match fact.silence {
                Silence::Never => false,
                Silence::AtValue(value) => rendered == value,
                Silence::AtValueUnlessAgreeing(value) => {
                    rendered == value
                        && raw_value(text, "cross_check").as_deref() != Some("\"agree\"")
                }
                Silence::WhenNoBannerArchitecture => banner_names_no_architecture(text),
            };

            // **A fact that cannot be mutated is not a fact that was checked.**
            // An earlier version let an empty mutation list stand for success,
            // so a classified key whose value has no mutation site -- an array
            // of strings, say -- would pass while the oracle had no reader for
            // it at all. That defeats the anti-drift guarantee this test is
            // for. Found by a review.
            assert!(
                !mutations.is_empty() || excused,
                "`{key}` is classified as read, and nothing here knows how to \
                 change the {rendered} it renders, so this host cannot show that \
                 anything reads it. Teach `corruptions` this value shape, or \
                 declare when the key is silent.\n\n--- the report ---\n{text}"
            );

            assert!(
                unread.is_empty() || excused,
                "nothing in {shape} reads `{key}` as any of {:?} -- {} of {} \
                 mutations of it went unnoticed. Either a rule that reads it has \
                 drifted from the renderer, or it never read this key. One of \
                 those mutations DELETES the key: if that is the one going \
                 unnoticed, the rule compares two present values and treats a \
                 dropped counterpart as silence.{}\n\n\
                 --- the report ---\n{text}",
                fact.reads,
                unread.len(),
                judged.len(),
                if fact.why.is_empty() {
                    String::new()
                } else {
                    format!(" The declared condition for silence is: {}.", fact.why)
                }
            );
        }

        if let Some((_, why)) = never {
            assert!(
                corruptions(text, key, None)
                    .iter()
                    .all(|corrupted| report_oracle::check(corrupted).is_empty()),
                "`{key}` is listed as having no second rendering -- {why} -- but \
                 the oracle now reports something when it is corrupted in {shape}. \
                 The exemption is stale; move it to FACTS.\n\n\
                 --- the report ---\n{text}"
            );
        }
    }
}

/// Whether any violation in `corrupted` is one of `fact`'s own correspondences.
///
/// Attribution matters: a violation from a neighbouring rule proves nothing
/// about whether THIS key is read. `AlarmWithAgreeingVerdict` is matched by the
/// key appearing in the alarm text, which is how the diagnostics rule names the
/// field it is complaining about.
fn reads_the_fact(corrupted: &str, fact: &Fact) -> bool {
    report_oracle::check(corrupted).iter().any(|violation| {
        match violation {
            report_oracle::Correspondence::ProseAndNdjsonDisagree { fact: named, .. } => {
                fact.reads.contains(named)
            }
            report_oracle::Correspondence::AlarmWithAgreeingVerdict { alarm, .. } => {
                alarm.contains(fact.key)
            }
            // The banner rule reports a processor count without naming a fact.
            report_oracle::Correspondence::BannerDisagreesWithBody { .. } => {
                fact.key == "processors"
            }
            report_oracle::Correspondence::UncaveatedClaimUnderDoubt { .. } => false,
            // Names its fact the same way `ProseAndNdjsonDisagree` does, and
            // for the same reason: it IS that comparison, reported when the
            // other side turned out not to be there.
            report_oracle::Correspondence::RenderedOnlyInProse { fact: named, .. } => {
                fact.reads.contains(named)
            }
            // Names its correspondence directly, so it attributes the same way.
            report_oracle::Correspondence::EvidenceMissingWithAgreeingVerdict { fact: named } => {
                fact.reads.contains(named)
            }
        }
    })
}

/// Whether no `host:` line in the report names an architecture.
///
/// **Asks the oracle rather than re-deciding.** This used to test
/// `line.contains("p/")`, which is what the oracle once did too -- and when the
/// oracle moved to reading the fingerprint's tokens by position, this copy
/// stayed behind. Measured: a failed-discovery banner of
/// `host:  UNKNOWN -- topology discovery failed: 16p/foo something opaque`
/// satisfied the substring test and not the oracle, so this helper reported that
/// the banner named an architecture, the `arch` silence exemption did not apply,
/// and `account_for_every_fact` failed on a report the oracle had accepted.
///
/// The instrument that measures whether a fact is read must not hold its own
/// opinion about what the oracle does. Found by a review.
fn banner_names_no_architecture(report: &str) -> bool {
    !report
        .lines()
        .filter(|line| line.starts_with("host:"))
        .any(|line| report_oracle::architecture_in_banner(line).is_some())
}

/// One corrupted copy of the report per mutation site in this key's value,
/// plus one with the key DELETED.
///
/// A container has one site per number it renders, so each nested property is
/// exercised on its own rather than standing behind the first.
///
/// **Deletion is a mutation, and leaving it out hid a whole class.** Every
/// mutation here used to rewrite a VALUE, so every rule was asked only "these
/// two renderings differ" and never "one of them is gone". The oracle's
/// comparisons were written as `if let (Some(prose), Some(json))`, which reads a
/// dropped field as silence -- and this instrument, the thing whose whole
/// purpose is to prove each fact is read, could not see it. Five sites were
/// found by review after three had already been fixed by review, which is the
/// signature of a class being patched instance by instance instead of swept.
///
/// Adding it here rather than beside each rule is deliberate: a rule added later
/// inherits the question automatically, which is the only version of this that
/// cannot rot.
fn corruptions(report: &str, key: &str, empty_replacement: Option<&str>) -> Vec<String> {
    let Some(original) = raw_value(report, key) else {
        return Vec::new();
    };

    // A `null` value has no corruption, but it can still be DELETED -- and the
    // absence of the key is a different claim from the presence of `null`.
    let deleted = report
        .replace(&format!("\"{key}\":{original},"), "")
        .replace(&format!(",\"{key}\":{original}"), "");

    if original == "null" {
        return vec![deleted];
    }

    let rewrite = |replacement: &str| {
        report.replace(
            &format!("\"{key}\":{original}"),
            &format!("\"{key}\":{replacement}"),
        )
    };

    if original.starts_with('"') {
        return vec![rewrite("\"x-corrupted\""), deleted];
    }

    if original.starts_with('[') || original.starts_with('{') {
        let sites: Vec<usize> = original
            .char_indices()
            .filter(|(index, character)| {
                character.is_ascii_digit()
                    && !original[..*index].ends_with(|previous: char| previous.is_ascii_digit())
            })
            .map(|(index, _)| index)
            .collect();

        if sites.is_empty() {
            // An empty container still has prose beside it, so substitute the
            // smallest value that disagrees with an empty one.
            let mut mutations = empty_replacement
                .map(|replacement| vec![rewrite(replacement)])
                .unwrap_or_default();
            mutations.push(deleted);
            return mutations;
        }

        let mut mutations: Vec<String> = sites
            .into_iter()
            .map(|at| {
                let mut copy = original.clone();
                let flipped = if &original[at..=at] == "0" { "1" } else { "0" };
                copy.replace_range(at..=at, flipped);
                rewrite(&copy)
            })
            .collect();

        // **A mutation that changes a NAME, not a number.** Every mutation above
        // changes a digit, so a rule that relates the SET of entries -- the
        // policy names, the cache levels -- is never exercised on its own: the
        // per-entry value comparisons fire for the same mutation and report the
        // key as read. Removing `compare_membership` for policy names would
        // leave this instrument green while that correspondence was dead.
        //
        // Renaming the first key is what reaches it. With the membership rule
        // present the rename is reported as a disagreement about which entries
        // exist; with it removed the prose name matches nothing, the per-entry
        // lookup returns `None`, and NOTHING is reported -- which this test then
        // fails on, as it should. Found by a review.
        // **Every distinct key, not just the first one.** Renaming only the
        // first reaches `level` in the cache array and never `domains`, so a
        // reader of the inner member could go blind while this stayed green:
        // measured, `{"level":1,"x-domains":8}` was accepted by the oracle
        // because the domain lookup required the two members to be adjacent and
        // in order, and nothing here renamed the second one to find out.
        for key in object_key_names(&original) {
            let from = format!("\"{key}\":");
            let to = format!("\"x-{key}\":");
            mutations.push(rewrite(&original.replacen(&from, &to, 1)));
        }

        mutations.push(deleted);

        return mutations;
    }

    vec![rewrite(if original == "0" { "1" } else { "0" }), deleted]
}

/// Every distinct key name a container value writes, in first-seen order.
///
/// Keys of nested objects included: they are what the renamed-member mutation
/// needs, and they are exactly the ones the top-level enumeration cannot see.
fn object_key_names(value: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();

    for (at, _) in value.match_indices("\":") {
        let Some(opening) = value[..at].rfind('"') else {
            continue;
        };
        let name = &value[opening + 1..at];

        if !name.is_empty() && !names.iter().any(|seen| seen == name) {
            names.push(name.to_owned());
        }
    }

    names
}

/// Every key the NDJSON line renders, in the order it renders them.
fn ndjson_keys(report: &str) -> Vec<String> {
    let Some(line) = report.lines().find(|line| line.starts_with('{')) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    let mut rest = line;
    let mut depth = 0_i32;

    while let Some(quote) = rest.find('"') {
        for character in rest[..quote].chars() {
            match character {
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            }
        }
        let after = &rest[quote + 1..];
        let Some(close) = after.find('"') else { break };
        let (name, tail) = after.split_at(close);
        let tail = &tail[1..];
        // Depth 1 only. A nested member is not enumerated as a fact of its own
        // -- it is part of its container's value, and so is covered by the
        // container's mutations instead. `a_nested_fact_nobody_classified_fails_the_accounting`
        // is the proof that this is a division of labour rather than a gap.
        if tail.starts_with(':') && depth == 1 {
            keys.push(name.to_owned());
        }
        rest = tail;
    }

    keys
}

/// The raw value the NDJSON renders for `key`, delimiters included.
fn raw_value(report: &str, key: &str) -> Option<String> {
    let line = report.lines().find(|line| line.starts_with('{'))?;
    let needle = format!("\"{key}\":");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];

    let end = if rest.starts_with('[') || rest.starts_with('{') {
        let mut depth = 0_i32;
        let mut close = None;
        for (index, character) in rest.char_indices() {
            match character {
                '[' | '{' => depth += 1,
                ']' | '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(index + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        close?
    } else if let Some(after) = rest.strip_prefix('"') {
        after.find('"')? + 2
    } else {
        rest.find([',', '}']).unwrap_or(rest.len())
    };

    Some(rest[..end].to_owned())
}

/// Every diagnostic this file writes goes through here.
///
/// The repository's output rule: once a second call site appears, where the text
/// goes stops being each call site's business. These are skip notes -- a reader
/// of a CI log needs them to tell "this host's shape meant the check could not
/// run" from "the check ran and found nothing", which are very different
/// readings of the same green result.
fn note(message: &str) {
    eprintln!("{message}");
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

#[test]
fn every_fact_is_accounted_for_on_every_representative_shape() {
    // The accounting, over the whole corpus rather than over this host alone.
    // Its classification -- which facts belong to a key, and when silence is
    // legitimate -- was written against one shape, so this is the first thing
    // that checks those declarations against the others.
    for shape in shapes() {
        let text = windows_platform_probes::topology_report::report(
            &banner_for(&shape.observation),
            &shape.observation,
        );

        account_for_every_fact(&text, shape.what);
    }
}

#[test]
fn a_failed_discovery_whose_error_mentions_a_count_is_still_accounted_for() {
    // **The instrument must not hold its own opinion about what the oracle
    // does.** `banner_names_no_architecture` decides whether the `arch` silence
    // exemption applies, and it used to answer with `line.contains("p/")` --
    // which agreed with the oracle until the oracle began reading the
    // fingerprint's tokens by position.
    //
    // `banner_line_for` interpolates a failed read's `io::Error` verbatim, so an
    // error mentioning a processor count puts `p/` in the banner of a report
    // that names no architecture at all. Measured before the fix: the helper
    // said the banner named one, the exemption did not apply, and the accounting
    // demanded that `arch` be read on a report the oracle had accepted -- a
    // valid unmeasured report failing the suite for a reason that was not about
    // the report.
    //
    // No corpus shape produces this banner and neither does this host, so
    // nothing else here would notice the two definitions drifting apart again.
    let error = || std::io::Error::other("16p/foo something opaque");
    let banner = windows_placement_probe::fingerprint::banner_line_for(&Err(error()));

    assert!(
        banner.contains("p/"),
        "this test is pointless unless the error text reaches the banner: {banner}"
    );

    let text = windows_platform_probes::topology_report::report_unmeasured(&banner, &error());

    assert!(
        banner_names_no_architecture(&text),
        "a failed read names no architecture, whatever its error text spells:\n{text}"
    );

    account_for_every_fact(
        &text,
        "a failed discovery whose error text contains a count",
    );
}

#[test]
fn a_nested_fact_nobody_classified_fails_the_accounting() {
    // **The enumeration is shallow; the GUARANTEE is not.** `ndjson_keys` records
    // only depth-1 keys, so nothing here lists `policies`' entries or a cache
    // object's `level` and `domains` -- which reads like a hole in a test called
    // `every_fact_the_renderer_publishes_is_accounted_for`, and a review read it
    // that way.
    //
    // It is not one, because read-or-unread is MEASURED rather than enumerated.
    // A nested field is part of its container's value, so `corruptions` mutates
    // it -- by digit, and now by renaming every distinct key -- and those
    // mutations must be noticed by a rule that names one of the container's
    // facts. A member nobody wrote a rule for is a mutation nobody notices.
    //
    // This test is that argument, executed. Without it the property holds by
    // reasoning about two functions that do not mention each other, which is
    // exactly the kind of claim this branch keeps finding to be false.
    // **Injection sites come from what this host actually rendered.** The first
    // version assumed a measured report with a level-1 cache and hard-coded
    // `"level":1,"domains":` -- the same host dependency a review had just
    // removed from another test in this file. A host whose discovery fails
    // renders neither container, and a measured one need not have an L1; there
    // the injection would silently not apply and the assertion below would fire
    // about a report that is perfectly legitimate.
    let (text, measured) = real_report();

    if !measured {
        // `report_unmeasured` publishes no container at all, so there is no
        // nested fact to leave unclassified and nothing for this test to say.
        return;
    }

    // **The injected member must be one NO rule reads, or this proves nothing.**
    // An earlier version also added `"by-latency":3` to the `policies` object --
    // but policy entries are NAMED, and `compare_membership` reads that name set,
    // so the extra entry is a fact the oracle covers rather than an unclassified
    // one. Measured: the injected report already carried a `policy names`
    // violation before any mutation, and the accounting then panicked on the
    // `reason` exemption ("the oracle now reports something when it is
    // corrupted") rather than on an unread fact. The assertion held and the
    // reason was wrong, which is the failure mode this whole file exists to
    // catch. Found by a review.
    //
    // A cache object's members are the case that works: the array is the only
    // container of objects whose members are read positionally rather than by
    // name, so a member nobody named is genuinely unclassified. Measured on the
    // same run: baseline `[]`, and the panic is
    // `nothing ... reads 'caches' ... 2 of 11 mutations went unnoticed`.
    let Some(cache) = first_cache_object(&text) else {
        // A measured report need not carry caches -- `no_levels_reported` is a
        // legitimate arm -- and then there is no object to nest a fact in.
        return;
    };

    let unclassified = [
        (
            "a number in a cache object",
            cache.replace('{', r#"{"latency":7,"#),
        ),
        (
            "a string in a cache object",
            cache.replace('{', r#"{"note":"x","#),
        ),
    ];

    for (what, to) in unclassified {
        let injected = text.replace(&cache, &to);

        assert_ne!(
            injected, text,
            "{what}: the injection did not apply, so this proves nothing"
        );

        // **And the report must still agree with itself before the mutation.**
        // If the injection itself creates a violation, every later assertion
        // sees it and this test can pass for a reason unrelated to the fact
        // being unread -- which is exactly how the `policies` case fooled it.
        assert_eq!(
            report_oracle::check(&injected),
            [],
            "{what}: injecting an unread fact must not itself be a violation, or \
             the accounting's panic below proves nothing about coverage"
        );

        let accounted = std::panic::catch_unwind(|| {
            account_for_every_fact(&injected, "a report carrying an unclassified nested fact")
        });

        assert!(
            accounted.is_err(),
            "{what} was published and no rule reads it, and the accounting accepted \
             the report anyway -- the shallow enumeration has become a real hole"
        );
    }
}

#[test]
fn one_blocks_caveat_does_not_excuse_another_blocks_claim() {
    // **A crossed shape: heterogeneous AND in doubt AND the `Level` cache arm.**
    // That arm writes the same "did not establish that the parse is whole"
    // sentence the heterogeneity caveat does, so a report-global search for the
    // caveat could be satisfied by the WRONG block and accept an uncaveated
    // hardware claim. A review predicted exactly that.
    //
    // Measured on this shape before the fix: it did NOT mask -- but only because
    // the renderer wraps the cache arm's sentence, so no single line carries the
    // whole token. The protection was an accident of where a line breaks, and
    // reflowing that sentence would have turned the oracle blind with nothing to
    // notice. The caveat search is now scoped to the claim's own block; this test
    // is what keeps that true.
    let mut crossed = base();
    crossed.numa_domains_only_in_cpu_sets = 2;
    crossed.cores = vec![
        CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 0,
            processors: 8,
        },
        CoreShape {
            simultaneous_multithreading: false,
            efficiency_class: 1,
            processors: 8,
        },
    ];

    let text = windows_platform_probes::topology_report::report(&banner_for(&crossed), &crossed);

    // The shape really is the crossed one, or this proves nothing.
    assert!(
        text.contains("(heterogeneous:"),
        "the shape must make the gated hardware claim:\n{text}"
    );
    assert!(
        text.contains("of the levels that decoded"),
        "and must carry the cache arm's own caveat, which is the masking \
         candidate:\n{text}"
    );
    assert_eq!(
        report_oracle::check(&text),
        [],
        "the crossed shape is legitimate and must be accepted as rendered"
    );

    // Remove ONLY the heterogeneity caveat. The cache arm's caveat stays.
    let caveat = text
        .lines()
        .find(|line| line.trim_start().starts_with("(This run did not establish"))
        .map(str::to_owned)
        .expect("the heterogeneity caveat must be present to be removed");
    let uncaveated = text.replace(&format!("{caveat}\n"), "");

    assert_ne!(uncaveated, text, "the caveat removal must apply");
    assert!(
        report_oracle::check(&uncaveated)
            .iter()
            .any(|found| matches!(
                found,
                report_oracle::Correspondence::UncaveatedClaimUnderDoubt {
                    claim: "heterogeneity",
                    ..
                }
            )),
        "another block's caveat must not answer for this claim: {:#?}",
        report_oracle::check(&uncaveated)
    );
}
