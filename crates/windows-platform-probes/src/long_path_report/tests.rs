// Copyright (c) Mike Grier.

//! Tests for the verdict.
//!
//! Every defect these pin was real, and every one reached a review round rather
//! than a build: the verdict turns data into English, and English can be false in
//! ways neither the compiler nor a passing test suite could see. Round after
//! round found them by reading output -- stated without a count, because the
//! count moved every time and a stale one here was itself a finding. This file
//! is the attempt to stop finding them that way.
//!
//! So each test is named for the wrong sentence it forbids, and asserts **both**
//! halves: that the right claim is made, and that the wrong one is not. Asserting
//! only the former would let a verdict pass by saying everything at once.
//!
//! `verdict` is a pure function of an [`Observation`], which is why this costs
//! nothing to test -- no filesystem, no Win32, no privileges, no current
//! directory to restore. That the probe's own apparatus is expensive to exercise
//! is not a reason for its conclusions to be untested.

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PATH_NOT_FOUND};

use super::verdict;
use crate::long_path::{Attempt, Observation, Shape};

const SHAPES: [Shape; 3] = [Shape::Plain, Shape::DotDot, Shape::ForwardSlash];

/// An attempt that landed under the ceiling.
fn below(shape: Shape, opened: bool) -> Attempt {
    Attempt {
        shape,
        resolved_len: 78,
        over_max_path: false,
        opened,
        error: if opened { 0 } else { ERROR_PATH_NOT_FOUND },
    }
}

/// An attempt that landed over the ceiling.
fn above(shape: Shape, opened: bool, error: u32) -> Attempt {
    Attempt {
        shape,
        resolved_len: 429,
        over_max_path: true,
        opened,
        error: if opened { 0 } else { error },
    }
}

/// The three below-ceiling attempts of a healthy run: every shape opened.
fn healthy_below() -> Vec<Attempt> {
    SHAPES.into_iter().map(|shape| below(shape, true)).collect()
}

/// The three above-ceiling attempts, all with the same outcome.
fn all_above(opened: bool, error: u32) -> Vec<Attempt> {
    SHAPES
        .into_iter()
        .map(|shape| above(shape, opened, error))
        .collect()
}

fn observation(
    manifest_aware: bool,
    registry_enabled: bool,
    attempts: Vec<Attempt>,
) -> Observation {
    Observation {
        manifest_aware,
        registry_enabled: Some(registry_enabled),
        attempts,
        apparatus_error: None,
    }
}

/// A run in which everything is in place and the ceiling genuinely lifted.
fn opted_in_and_lifted() -> Observation {
    let mut attempts = healthy_below();
    attempts.extend(all_above(true, 0));
    observation(true, true, attempts)
}

#[test]
fn a_refusal_without_the_opt_in_is_not_evidence_against_the_documentation() {
    // The flagship line of `probe-long-path-unaware` on every correctly
    // configured host, and it said "The documented reading is wrong for this
    // configuration". The documented reading is about the *opt-in*; a binary with
    // no manifest never opted in, so `MAX_PATH` applying to it is what `MAX_PATH`
    // means. The run that most exactly confirms the documentation was reporting
    // that it refutes it.
    let mut attempts = healthy_below();
    attempts.extend(all_above(false, ERROR_PATH_NOT_FOUND));
    let text = verdict(&observation(false, true, attempts));

    assert!(
        text.contains("BASELINE"),
        "an un-opted-in refusal is the baseline the opted-in half is read against: {text}"
    );
    assert!(
        !text.contains("documented reading is wrong"),
        "nothing here bears on the documentation, which describes the opted-in case: {text}"
    );
    assert!(
        text.contains("longPathAware"),
        "the report must name which half was missing, or the reader cannot tell: {text}"
    );
}

#[test]
fn a_refusal_with_the_opt_in_in_effect_is_evidence_against_the_documentation() {
    // The control for the test above. Without it, deleting the whole conclusion
    // would leave that one green -- it asserts an absence, and an absence is
    // satisfied by saying nothing at all.
    let mut attempts = healthy_below();
    attempts.extend(all_above(false, ERROR_PATH_NOT_FOUND));
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("documented reading is wrong"),
        "with both halves in effect a refusal *is* the counter-example: {text}"
    );
    assert!(
        !text.contains("BASELINE"),
        "this run opted in, so it is not the baseline: {text}"
    );
}

#[test]
fn a_ceiling_that_lifts_without_the_opt_in_is_not_credited_to_the_opt_in() {
    // The same error facing the other way. A host that lifts the ceiling for a
    // binary that never opted in is a stranger result than either headline
    // verdict describes, and reporting it as "MAX_PATH was lifted" would credit
    // an opt-in that was switched off.
    let text = verdict(&opted_in_and_lifted_but(false, true));

    assert!(
        text.contains("UNEXPECTED"),
        "lifting without the opt-in is not the ordinary lifted verdict: {text}"
    );
    assert!(
        !text.contains("No evidence of a prefix-then-parse"),
        "that conclusion is about how the opt-in behaves, and it was not on: {text}"
    );
}

/// [`opted_in_and_lifted`] with the two halves of the opt-in overridden.
fn opted_in_and_lifted_but(manifest_aware: bool, registry_enabled: bool) -> Observation {
    Observation {
        manifest_aware,
        registry_enabled: Some(registry_enabled),
        ..opted_in_and_lifted()
    }
}

#[test]
fn a_shape_never_tried_below_the_ceiling_is_not_reported_as_a_fault() {
    // Measured at a 215-character `%TEMP%`, before the 200-unit limit existed:
    // every attempt opened, nothing failed, and the report announced "an
    // apparatus or platform problem". The shapes do not cross the ceiling
    // together -- `..` carries an extra `\b\..` -- so a deep enough current
    // directory pushed it over while plain was still under.
    //
    // No longer reachable from `%TEMP%`, which is now capped well below that.
    // Kept for the same reason as the case below: a change to `shallow` or
    // `SEGMENT` would put it back, and nothing else would notice.
    let attempts = vec![
        below(Shape::Plain, true),
        // No below-ceiling attempt for `..` at all: it is over even when short.
        above(Shape::DotDot, true, 0),
        below(Shape::ForwardSlash, true),
        above(Shape::Plain, true, 0),
        above(Shape::DotDot, true, 0),
        above(Shape::ForwardSlash, true, 0),
    ];
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("never tried"),
        "the missing baseline is the fact, and it is not a failure: {text}"
    );
    // Only the live half of this is kept. The original also forbade "apparatus or
    // platform problem", which was the pre-fix wording -- and since that string no
    // longer exists anywhere the renderer can emit, the conjunct was trivially
    // true and would have stayed green under any regression. An assertion that
    // cannot fail is worse than none: it reads as coverage.
    assert!(
        !text.contains("apparatus is wrong"),
        "nothing failed, so naming a fault sends the reader hunting for one: {text}"
    );
}

#[test]
fn a_shape_that_failed_below_the_ceiling_is_reported_as_a_fault() {
    // The other half of the split. Here something really did fail where it should
    // not have, and the report must say so rather than blaming the ceiling.
    let attempts = vec![
        below(Shape::Plain, true),
        below(Shape::DotDot, false),
        below(Shape::ForwardSlash, true),
        above(Shape::Plain, false, ERROR_PATH_NOT_FOUND),
        above(Shape::DotDot, false, ERROR_PATH_NOT_FOUND),
        above(Shape::ForwardSlash, false, ERROR_PATH_NOT_FOUND),
    ];
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("apparatus is wrong"),
        "a shape failing below the ceiling is a broken apparatus: {text}"
    );
    assert!(
        !text.contains("documented reading is wrong"),
        "a broken apparatus cannot support a claim about the documentation: {text}"
    );
}

#[test]
fn a_long_failure_that_is_not_a_length_refusal_yields_no_verdict() {
    // Both headline verdicts once branched on "did not open" alone, so a sharing
    // violation or an access denial produced a claim about `MAX_PATH` from
    // evidence bearing on neither. The crate already owned the discriminator --
    // `is_refusal` -- and was spending it on a parenthetical in the table.
    let mut attempts = healthy_below();
    attempts.extend(all_above(false, ERROR_ACCESS_DENIED));
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("NO VERDICT") && text.contains("not a length refusal"),
        "an access denial says nothing about the ceiling: {text}"
    );
    assert!(
        !text.contains("documented reading is wrong"),
        "that headline needs a length refusal, which this is not: {text}"
    );
    // In context, not as a bare number: `ERROR_ACCESS_DENIED.to_string()` is "5",
    // and a substring search for "5" is satisfied by any verdict that happens to
    // contain the digit -- including the ceiling, 259.
    assert!(
        text.contains(&format!("(error {ERROR_ACCESS_DENIED})")),
        "the unexpected error must be named, or it cannot be chased: {text}"
    );
}

#[test]
fn a_reference_shape_never_tried_above_the_ceiling_yields_no_verdict() {
    // "No long plain attempt opened" was read as "plain was refused", conflating a
    // refusal with an attempt that was never made. Reproduced by forcing the deep
    // level to 21, which is the only way to reach it: **no `%TEMP%` length can**,
    // and saying otherwise would send the next reader looking for a host
    // configuration that does not exist.
    //
    // The deep relative path is a fixed 370 units -- 40 levels of an 8-unit
    // `SEGMENT` plus separators plus `target.txt` -- so the deep plain attempt is
    // never below about 392 against a ceiling of 259. With the shipped `deep = 40`
    // this state is unreachable, and the guard is defence against a future change
    // to `deep` or `SEGMENT` rather than against anything a host can do. That is
    // exactly why it is worth a test: nothing else would notice it rotting.
    let attempts = vec![
        below(Shape::Plain, true),
        below(Shape::DotDot, true),
        below(Shape::ForwardSlash, true),
        // A shallower tree: plain still lands under the ceiling at depth, so it is
        // never tested above it.
        below(Shape::Plain, true),
        above(Shape::DotDot, false, ERROR_PATH_NOT_FOUND),
        below(Shape::ForwardSlash, true),
    ];
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("NO VERDICT"),
        "with no plain attempt above the ceiling there is nothing to conclude: {text}"
    );
    assert!(
        !text.contains("even the plain shape was refused"),
        "plain was never tried above the ceiling, so it was not refused there: {text}"
    );
}

#[test]
fn the_sharp_edge_names_only_shapes_that_worked_below_the_ceiling() {
    // The probe's headline finding. It claims a shape "stopped resolving past the
    // ceiling while working below it", and for a long time computed that from the
    // long attempts alone -- so a shape that failed at *both* lengths would have
    // been reported as the sharp edge, which is the opposite of what happened.
    let attempts = vec![
        below(Shape::Plain, true),
        below(Shape::DotDot, true),
        below(Shape::ForwardSlash, true),
        above(Shape::Plain, true, 0),
        above(Shape::DotDot, false, ERROR_PATH_NOT_FOUND),
        above(Shape::ForwardSlash, true, 0),
    ];
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("SHARP EDGE") && text.contains(Shape::DotDot.label()),
        "`..` worked below and failed above, which is exactly the sharp edge: {text}"
    );
    assert!(
        !text.contains(Shape::ForwardSlash.label()),
        "forward slashes resolved past the ceiling, so naming them is false: {text}"
    );
}

#[test]
fn a_fully_opted_in_run_that_lifts_reports_no_prefix_then_parse() {
    // The ordinary success, and the shape of the aware binary's real output. Its
    // presence keeps the tests above honest: they forbid wrong sentences, and
    // without this one the whole function could satisfy them by never concluding.
    let text = verdict(&opted_in_and_lifted());

    assert!(
        text.contains("No evidence of a prefix-then-parse"),
        "every shape resolved past the ceiling, which is the negative result: {text}"
    );
    assert!(
        !text.contains("NO VERDICT") && !text.contains("SHARP EDGE"),
        "nothing was withheld and nothing broke: {text}"
    );
}

#[test]
fn a_run_with_no_long_attempt_tested_nothing_and_says_so() {
    // The degenerate case: no attempt reached the ceiling at all. Only a shallower
    // tree can produce it -- a short `%TEMP%` cannot, because the deep attempts
    // carry a fixed 370 units whatever the current directory costs. There is no
    // finding here in either direction.
    let text = verdict(&observation(true, true, healthy_below()));

    assert!(
        text.contains("tested nothing"),
        "without an attempt past the ceiling the run establishes nothing: {text}"
    );
    assert!(
        !text.contains("lifted") && !text.contains("SHARP EDGE"),
        "no conclusion is available from attempts that never crossed: {text}"
    );
}

#[test]
fn an_absent_registry_half_is_named_even_when_the_manifest_is_present() {
    // The state `probe-long-path-aware` reaches on a machine whose administrator
    // never set `LongPathsEnabled` -- which includes a stock CI runner, so this is
    // the aware binary's likely output wherever this repository builds. The
    // manifest alone does not opt in, so a refusal here is still the baseline, and
    // the report has to name the *registry* as the missing half or the reader will
    // check the one thing that is present.
    let mut attempts = healthy_below();
    attempts.extend(all_above(false, ERROR_PATH_NOT_FOUND));
    let text = verdict(&observation(true, false, attempts));

    assert!(
        text.contains("BASELINE"),
        "one half is not an opt-in, so this is still the baseline: {text}"
    );
    assert!(
        text.contains("LongPathsEnabled"),
        "the missing half is the machine setting, and must be the one named: {text}"
    );
    assert!(
        !text.contains("no `longPathAware` manifest"),
        "this binary *has* the manifest; naming it sends the reader to the wrong half: {text}"
    );
}

#[test]
fn both_halves_absent_are_reported_as_neither_rather_than_as_one() {
    // The commonest real-world state for the un-opted-in half: a machine without
    // the registry value running a binary without the manifest. Naming only one
    // would imply the other was in place.
    let mut attempts = healthy_below();
    attempts.extend(all_above(false, ERROR_PATH_NOT_FOUND));
    let text = verdict(&observation(false, false, attempts));

    assert!(
        text.contains("BASELINE") && text.contains("neither half"),
        "with both absent the report must say so, not pick one: {text}"
    );
    assert!(
        !text.contains("LongPathsEnabled") && !text.contains("no `longPathAware` manifest"),
        "naming a single half implies the other was present: {text}"
    );
}

#[test]
fn a_broken_apparatus_outranks_a_missing_baseline() {
    // Both conditions at once: one shape failed below the ceiling (the apparatus
    // is wrong) while another was never tried below it (no baseline). The order
    // matters because only one message is emitted, and the apparatus fault is the
    // one that invalidates the whole run -- reporting the missing baseline instead
    // would describe a consequence and hide the cause.
    let attempts = vec![
        below(Shape::Plain, false),
        // No below-ceiling attempt for `..` at all.
        above(Shape::DotDot, true, 0),
        below(Shape::ForwardSlash, true),
        above(Shape::Plain, false, ERROR_PATH_NOT_FOUND),
        above(Shape::DotDot, true, 0),
        above(Shape::ForwardSlash, true, 0),
    ];
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("apparatus is wrong") && text.contains(Shape::Plain.label()),
        "the fault is the cause and must be what the reader is told: {text}"
    );
    assert!(
        !text.contains("never tried"),
        "the missing baseline is a consequence here, and would hide the fault: {text}"
    );
}

#[test]
fn a_run_with_nothing_below_the_ceiling_claims_nothing_about_the_apparatus() {
    // The state every other test in this file misses, and the reason it lasted
    // as long as it did: each of them seeds at least one below-ceiling attempt, so the
    // sentence "every attempt that did land below the ceiling opened" always had
    // something to be true *of*. With none, it is vacuously true and was still
    // being offered as affirmative evidence that the apparatus was sound.
    //
    // Measured rather than imagined, at a 226-character `%TEMP%` -- but only
    // before the 200-unit limit existed. Such a temporary directory put even the
    // shallow attempts over the ceiling, and the unaware binary refused all six
    // while the report announced that everything below the ceiling had opened.
    //
    // The limit now refuses that configuration up front, so no `%TEMP%` reaches
    // this state; it takes a change to `shallow` or `SEGMENT`. Kept because that
    // is exactly the change nothing else would catch.
    let attempts = all_above(false, ERROR_PATH_NOT_FOUND);
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("No attempt landed below"),
        "with nothing under the ceiling that is the fact to report: {text}"
    );
    assert!(
        text.contains("says nothing about the apparatus"),
        "an untested apparatus must be described as untested, not as sound: {text}"
    );
    assert!(
        !text.contains("did land below the ceiling opened"),
        "nothing landed below the ceiling, so that claim has no subject: {text}"
    );
    // `apparatus fault:` and not `not an apparatus fault`: the latter spans a line
    // break in the emitted text (`not an\n   apparatus fault:`), so asserting it
    // would be an assertion that cannot fail -- which is the defect an earlier
    // round caught in this same file, and which I reintroduced writing this pair.
    assert!(
        !text.contains("apparatus fault:"),
        "the run never exercised the apparatus, so it cannot clear it: {text}"
    );
}

#[test]
fn a_run_with_something_below_the_ceiling_still_clears_the_apparatus() {
    // The control. Without it, deleting the non-vacuous branch entirely would
    // leave the test above green, since it asserts absences.
    let attempts = vec![
        below(Shape::Plain, true),
        above(Shape::DotDot, false, ERROR_PATH_NOT_FOUND),
        below(Shape::ForwardSlash, true),
        above(Shape::Plain, false, ERROR_PATH_NOT_FOUND),
        above(Shape::ForwardSlash, false, ERROR_PATH_NOT_FOUND),
    ];
    let text = verdict(&observation(true, true, attempts));

    assert!(
        text.contains("did land below the ceiling opened") && text.contains("apparatus fault:"),
        "plain and forward slashes opened below, so the apparatus is demonstrably sound: {text}"
    );
    assert!(
        !text.contains("No attempt landed below"),
        "two attempts did land below the ceiling: {text}"
    );
}

// ---------------------------------------------------------------------------
// `body` -- everything the report says once the measurement is in hand.
//
// Untestable until `render` was split, because `render` performs the
// measurement itself and so cannot be called without building a directory tree
// and moving the process's current directory. What follows is not formatting
// coverage: some of it fixes defects a review round had to find by reading a
// probe's output, which is the slowest possible detector.
// ---------------------------------------------------------------------------

/// The report body for an observation, as a string.
fn body_of(observation: &Observation) -> String {
    let mut out = String::new();
    super::body(&mut out, observation);
    out
}

#[test]
fn the_ceiling_column_prints_the_number_it_actually_compares() {
    // The header read `> MAX` beside a module defining `MAX_PATH` as 260, while
    // the comparison was against 259. Those differ at exactly one length -- the
    // length this probe exists to be exact about -- so a row could read `260 yes`
    // under a heading meaning "over 260".
    let text = body_of(&observation(true, true, healthy_below()));

    assert!(
        text.contains("> 259"),
        "the column must name the ceiling it compares against: {text}"
    );
    assert!(
        !text.contains("> MAX"),
        "`> MAX` is read as 'over 260', which is the one wrong reading: {text}"
    );
}

#[test]
fn a_not_found_below_the_ceiling_is_not_called_a_length_refusal() {
    // `is_refusal` separates a length rejection from a genuine absence and says
    // nothing about length itself. Applied to every failed attempt, it annotated
    // under-ceiling rows "so this is the length refusal" -- blaming the ceiling
    // for a file that was simply missing.
    let attempts = vec![below(Shape::Plain, false)];
    let text = body_of(&observation(true, true, attempts));

    assert!(
        text.contains("BELOW the ceiling") && text.contains("apparatus is wrong"),
        "a not-found under the ceiling is an apparatus fault: {text}"
    );
    assert!(
        !text.contains("this is the length refusal"),
        "length is the one thing that annotation asserts and this row is not over: {text}"
    );
}

#[test]
fn a_not_found_above_the_ceiling_is_called_a_length_refusal() {
    // The control for the test above: without it, deleting the annotation
    // entirely would leave that one green, since it asserts an absence.
    let attempts = vec![above(Shape::Plain, false, ERROR_PATH_NOT_FOUND)];
    let text = body_of(&observation(true, true, attempts));

    assert!(
        text.contains("this is the length refusal"),
        "over the ceiling, a not-found on a file that exists is the length refusal: {text}"
    );
}

#[test]
fn an_absent_registry_half_is_flagged_before_any_row_is_read() {
    // The machine half is a precondition for the whole run, so the warning has to
    // come before the table rather than after it -- a reader who has already
    // interpreted the rows has been misled by the time a footnote arrives.
    let text = body_of(&observation(true, false, healthy_below()));

    let warning = text
        .find("machine half of the opt-in is absent")
        .expect("the absent registry half must be reported");
    let table = text
        .find("shape")
        .expect("the table header must be present");
    assert!(
        warning < table,
        "the warning must precede the rows it qualifies: {text}"
    );
}

#[test]
fn an_apparatus_failure_stops_the_report_before_the_rows() {
    // When the apparatus failed the attempts are meaningless, and printing them
    // under a heading that invites comparison is worse than printing nothing:
    // the numbers look like findings.
    let attempts = vec![above(Shape::Plain, false, ERROR_PATH_NOT_FOUND)];
    let observation = Observation {
        apparatus_error: Some("could not build the tree".to_string()),
        ..observation(true, true, attempts)
    };
    let text = body_of(&observation);

    assert!(
        text.contains("APPARATUS FAILED") && text.contains("could not build the tree"),
        "the failure and its cause must both be reported: {text}"
    );
    assert!(
        !text.contains("shape") && !text.contains("REFUSED"),
        "rows gathered by a broken apparatus must not be shown at all: {text}"
    );
}

#[test]
fn a_refused_run_claims_nothing_about_the_registry() {
    // A run refused for an over-long temporary directory never issues the
    // registry query, because reading it spawns a process and that is one of the
    // calls that hangs. Reporting the unread flag as `false` made the report say
    // `LongPathsEnabled : unset or 0` and "the machine half of the opt-in is
    // absent" on a host where the value is 1 -- a measured-sounding claim about a
    // query that was never made, immediately above an apparatus error saying
    // nothing below described the machine.
    let observation = Observation {
        manifest_aware: true,
        registry_enabled: None,
        attempts: Vec::new(),
        apparatus_error: Some("%TMP% is too long".to_string()),
    };
    let text = body_of(&observation);

    assert!(
        text.contains("not consulted"),
        "an unread setting must be reported as unread: {text}"
    );
    assert!(
        !text.contains("unset or 0"),
        "that is a claim about the machine, and no query was issued: {text}"
    );
    assert!(
        !text.contains("machine half of the opt-in is absent"),
        "warning that it is absent asserts the same unmade measurement: {text}"
    );
    assert!(
        text.contains("APPARATUS FAILED"),
        "the reason the run established nothing must still be reported: {text}"
    );
}

#[test]
fn a_registry_half_that_was_read_and_is_off_is_still_flagged() {
    // The control. `Some(false)` is a real measurement and must keep warning --
    // without this, suppressing the warning entirely would satisfy the test above.
    let text = body_of(&observation(true, false, healthy_below()));

    assert!(
        text.contains("machine half of the opt-in is absent"),
        "a registry half that was read and is off is a real finding: {text}"
    );
    assert!(
        text.contains("unset or 0") && !text.contains("not consulted"),
        "it was consulted, and the report should say what it found: {text}"
    );
}

#[test]
fn a_shape_that_cannot_survive_verbatim_parsing_is_not_called_a_length_refusal() {
    // The annotation for a not-found past the ceiling used to say "the target
    // provably exists, so this is the length refusal" for every shape. For `..`
    // and forward slashes that is the ambiguity the probe exists to resolve
    // rather than a fact about the run: if the system prepends `\\?\` past the
    // ceiling, those shapes stop being resolved and the path as written names
    // something never created -- a genuine absence, and the sharp edge itself.
    // Calling it the length refusal files the finding as its own control.
    let attempts = vec![above(Shape::DotDot, false, ERROR_PATH_NOT_FOUND)];
    let text = body_of(&observation(true, true, attempts));

    assert!(
        text.contains("either the length refusal or this shape ceasing to"),
        "for a shape `\\\\?\\` would break, the cause is genuinely undecided here: {text}"
    );
    assert!(
        !text.contains("so this is the length refusal"),
        "asserting the length refusal would rule out the sharp edge by fiat: {text}"
    );
}
