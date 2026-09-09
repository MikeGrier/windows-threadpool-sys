// Copyright (c) Mike Grier.

//! Renders one long-path run. Shared by the two binaries, which differ only in
//! whether their manifest declares `longPathAware`.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. See this crate's DESIGN-NOTES.md.

use crate::long_path::{MAX_PATH_CONTENT, Observation, Shape, is_refusal};

#[cfg(test)]
mod tests;

/// The probe's whole report, composed into `out`.
///
/// Takes the flag rather than a finished [`Observation`], and runs the
/// measurement itself, so that this is the crate's ordinary renderer shape:
/// `main` is one [`crate::report::emit_report`] call and everything the probe
/// knows is composed *inside* it.
///
/// The difference is not cosmetic. Passing an `&Observation` would mean the
/// measurement ran while evaluating the argument -- before this function is
/// entered, with `out` still empty -- so a panic anywhere in it would print
/// nothing at all, not even the banner that every captured report is supposed to
/// carry. Composing the banner and the header first makes the buffer worth
/// emitting from the moment measurement begins.
pub fn render(out: &mut dyn std::fmt::Write, manifest_aware: bool) {
    preamble(out);
    // Everything above is already in `out`, so the report survives whatever this
    // does.
    body(out, &crate::long_path::measure(manifest_aware));
}

/// Everything that can be written before the measurement runs.
///
/// Split out so [`body`] can be tested. The ordering is the point rather than an
/// artefact: this must reach `out` before [`crate::long_path::measure`] is
/// called, or a panic inside the measurement prints nothing at all.
fn preamble(out: &mut dyn std::fmt::Write) {
    // First line of the report, and part of the composed text rather than
    // written out here: a captured report must carry the line naming the
    // machine that produced it, and the taint marker with it.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    let _ = writeln!(
        out,
        "== does the long-path opt-in lift MAX_PATH for a relative path? ==\n"
    );
}

/// Everything the report says once the measurement is in hand.
///
/// Takes a finished [`Observation`] so it is a pure function of data, which is
/// the whole reason it exists apart from [`render`]: `render` cannot be called
/// without building a directory tree and moving the process's current directory,
/// so anything left inside it is untestable. What lives here is not formatting --
/// it is the length-refusal gating, the ceiling the column compares against, the
/// registry warning, and the apparatus-error early return, two of which are
/// fixes for defects a review round had to find by reading output.
fn body(out: &mut dyn std::fmt::Write, observation: &Observation) {
    let _ = writeln!(
        out,
        "manifest longPathAware : {}",
        if observation.manifest_aware {
            "yes"
        } else {
            "no"
        }
    );
    let _ = writeln!(
        out,
        "LongPathsEnabled       : {}",
        match observation.registry_enabled {
            Some(true) => "1",
            Some(false) => "unset or 0",
            None => "not consulted (the run was refused first)",
        }
    );

    // Only when it was read and was off. `None` means the query never happened,
    // and warning that "the machine half of the opt-in is absent" on that basis
    // is a claim about the machine drawn from a measurement that was not taken --
    // observed reporting `unset or 0` on a host whose value is 1.
    if observation.registry_enabled == Some(false) {
        let _ = writeln!(
            out,
            "\n*** The machine half of the opt-in is absent, so this run measures the\n\
             *** un-opted-in case whatever the manifest says. The rows below are still\n\
             *** real, but they answer a different question than the one intended."
        );
    }

    if let Some(error) = &observation.apparatus_error {
        let _ = writeln!(
            out,
            "\n*** APPARATUS FAILED: {error}\n\
             *** Nothing below says anything about the machine."
        );
        return;
    }

    // The ceiling is printed rather than named. `> MAX` beside a module that
    // defines `MAX_PATH` as 260 reads as "over 260", and the number that actually
    // governs is 259 -- the terminator takes the last unit. Those differ at
    // exactly one length, which is exactly the length this probe exists to be
    // right about.
    let _ = writeln!(
        out,
        "\n{:<18} {:>8} {:>8}  {:<10} error",
        "shape",
        "resolved",
        format!("> {MAX_PATH_CONTENT}"),
        "result"
    );
    for attempt in &observation.attempts {
        let _ = writeln!(
            out,
            "{:<18} {:>8} {:>8}  {:<10} {}",
            attempt.shape.label(),
            attempt.resolved_len,
            if attempt.over_max_path { "yes" } else { "no" },
            if attempt.opened { "opened" } else { "REFUSED" },
            if attempt.opened {
                String::new()
            } else {
                // `is_refusal` distinguishes a length rejection from a genuine
                // absence, and says nothing about length itself -- so it is only
                // the length refusal when the row is *also* over the ceiling.
                // Below it, the same not-found means the apparatus is wrong, and
                // labelling that "the length refusal" would blame the ceiling for
                // a file that was simply missing.
                //
                // Over the ceiling it is only *unambiguously* the length refusal
                // for a shape that survives verbatim parsing. For the other two it
                // is exactly the ambiguity this probe exists to resolve: if the
                // system prepends `\\?\` past the ceiling, `..` and `/` stop being
                // resolved, and the path as written then names something that was
                // never created -- a genuine absence, and the sharp edge itself.
                // Calling that "the length refusal" would file the finding as its
                // own control.
                format!(
                    "{}{}",
                    attempt.error,
                    match (
                        attempt.over_max_path,
                        is_refusal(attempt),
                        attempt.shape.survives_verbatim_parsing()
                    ) {
                        (true, true, true) =>
                            " (not-found; this shape survives `\\\\?\\` parsing and the target \
                             exists, so this is the length refusal)",
                        (true, true, false) =>
                            " (not-found; either the length refusal or this shape ceasing to \
                             resolve past the ceiling -- the verdict below decides which)",
                        (false, true, _) =>
                            " (not-found BELOW the ceiling; the apparatus is wrong, not the path length)",
                        _ => "",
                    }
                )
            }
        );
    }

    let _ = writeln!(out, "\n{}", verdict(observation));
}

/// What the rows mean, stated rather than left for the reader to infer.
fn verdict(observation: &Observation) -> String {
    let long: Vec<_> = observation
        .attempts
        .iter()
        .filter(|attempt| attempt.over_max_path)
        .collect();
    if long.is_empty() {
        return "-- no attempt exceeded MAX_PATH, so this run tested nothing.".to_string();
    }

    // The control, and it is not optional. Every verdict below is a claim about a
    // *change* at the ceiling -- "stopped resolving past it while working below
    // it" -- and a change needs both sides. Reading only the long attempts would
    // let a shape that never worked at any length be reported as the sharp edge,
    // which is this probe's headline finding and the opposite of what happened.
    //
    // Two different situations, and they must not share a sentence.
    //
    // A shape with **no** below-ceiling attempt was never tried there. The shallow
    // depth is meant to land under the ceiling for every shape, and the shapes do
    // not cross together -- `..` carries an extra `\b\..`, so it reaches the
    // ceiling five units sooner than plain. A current directory long enough to
    // separate them puts `..` over while plain is still under.
    //
    // A shape that *was* tried below the ceiling and did not open is the opposite:
    // the apparatus is broken.
    //
    // Collapsing the two announced "an apparatus or platform problem" on runs
    // where every attempt opened and nothing failed at all. Measured with a
    // 215-character `%TEMP%`, where all six attempts opened and the report still
    // named a problem -- the report asserting a cause it did not establish, which
    // is the one thing this verdict exists to avoid.
    //
    // That measurement predates the limit on `%TMP%`/`%TEMP%` length, which now
    // refuses a temporary directory long enough to reach either state. Both are
    // kept because a change to `shallow` or `SEGMENT` would put them back, and
    // nothing else would notice.
    //
    // Paired on `over_max_path` rather than on depth, since it is the ceiling
    // rather than the depth that decides whether an attempt is a control.
    // Whether anything *failed* is a separate question, answered below: with some
    // attempts under the ceiling the run can say the apparatus is sound, and with
    // none it cannot say anything about the apparatus at all.
    let tried_below = |shape: Shape| {
        observation
            .attempts
            .iter()
            .any(|attempt| attempt.shape == shape && !attempt.over_max_path)
    };
    let opened_below = |shape: Shape| {
        observation
            .attempts
            .iter()
            .any(|attempt| attempt.shape == shape && !attempt.over_max_path && attempt.opened)
    };
    let shapes = [Shape::Plain, Shape::DotDot, Shape::ForwardSlash];

    let broken: Vec<&str> = shapes
        .into_iter()
        .filter(|shape| tried_below(*shape) && !opened_below(*shape))
        .map(Shape::label)
        .collect();
    if !broken.is_empty() {
        return format!(
            "-- NO VERDICT. These shapes were tried below the ceiling and did not open:\n   \
             {}. The apparatus is wrong, so nothing here is a finding about\n   \
             the ceiling.",
            broken.join(", ")
        );
    }

    let untried: Vec<&str> = shapes
        .into_iter()
        .filter(|shape| !tried_below(*shape))
        .map(Shape::label)
        .collect();
    if !untried.is_empty() {
        // Whether *anything* landed below the ceiling, which decides whether this
        // report can say a word about the apparatus.
        //
        // "Every attempt that did land below the ceiling opened" is true when some
        // did, and vacuously true when none did -- and a vacuous truth offered as
        // affirmative evidence is the same defect this whole cascade exists to
        // prevent. Measured with a 226-character `%TEMP%`: all six attempts were
        // over the ceiling and every one was REFUSED, and the report told the
        // reader that everything below the ceiling had opened. The rows directly
        // above it said otherwise. That length is now refused outright, so this
        // guard defends against a change to `shallow` or `SEGMENT` rather than
        // against a host.
        let anything_below = observation
            .attempts
            .iter()
            .any(|attempt| !attempt.over_max_path);

        if !anything_below {
            return format!(
                "-- NO VERDICT. No attempt landed below {MAX_PATH_CONTENT} on this run, so there is no\n   \
                 baseline to read the long case against for any shape: {}.\n   \
                 This says nothing about the apparatus either way -- it was never\n   \
                 exercised under the ceiling. The shallow depth is meant to land\n   \
                 there, so either it or the segment length has been changed.",
                untried.join(", ")
            );
        }

        return format!(
            "-- NO VERDICT. These shapes were never tried *below* {MAX_PATH_CONTENT} on this run,\n   \
             so there is no baseline to read the long case against: {}.\n   \
             Every attempt that did land below the ceiling opened, so this is not an\n   \
             apparatus fault: the shallow depth is meant to land under the ceiling\n   \
             for every shape, and for these it did not.",
            untried.join(", ")
        );
    }

    // The reference shape must have been tried *above* the ceiling too, or there
    // is nothing to conclude about lifting. Reading "no long plain attempt
    // opened" as "plain was refused" conflates a refusal with an attempt that was
    // never made.
    //
    // Measured, not hypothesised -- but reached by changing a constant, because
    // no host configuration can produce it. This state needs the *deep* plain
    // attempt to be under the ceiling, and the deep relative path is a fixed 370
    // units (40 levels of an 8-unit `SEGMENT`, separators, `target.txt`), so it
    // resolves to at least ~392 whatever the temporary directory costs.
    //
    // Forced by setting the deep level to 21 on the development host, the run
    // produced plain at 258 opened and `..` at 263 refused -- and the verdict
    // announced "even the plain shape was refused past the ceiling", which the
    // run had not observed and which the table directly contradicted.
    //
    // The five-unit straddle that `..` creates belongs to the guard above, not
    // this one; repeating it here would point the next reader at a host setting
    // that cannot reach this branch.
    let plain_long: Vec<_> = long
        .iter()
        .filter(|attempt| attempt.shape == Shape::Plain)
        .collect();
    if plain_long.is_empty() {
        return "-- NO VERDICT. No plain-shape attempt exceeded MAX_PATH, so the reference\n   \
                shape was never tested against the ceiling on this run and there is\n   \
                nothing to say about lifting."
            .to_string();
    }

    // *Why* a long attempt failed, which neither guard above constrains. Both
    // verdicts below are claims about the ceiling, and "did not open" is not
    // evidence about the ceiling: a sharing violation, an access denial, or an
    // antivirus hold on the target fails the open without the length being
    // involved at all. `apparatus_error` does not catch it either, because a
    // failed attempt is a result rather than a broken apparatus.
    //
    // The crate already owns the discriminator -- `is_refusal` exists to separate
    // a length rejection from a genuine absence -- and it was being spent on a
    // parenthetical in the table while the verdict that carries the finding
    // ignored it.
    let unexplained: Vec<String> = long
        .iter()
        .filter(|attempt| !attempt.opened && !is_refusal(attempt))
        .map(|attempt| format!("{} (error {})", attempt.shape.label(), attempt.error))
        .collect();
    if !unexplained.is_empty() {
        return format!(
            "-- NO VERDICT. These attempts past the ceiling failed for something that is\n   \
             not a length refusal, so nothing here bears on MAX_PATH: {}.",
            unexplained.join(", ")
        );
    }

    // Sound only because of the three checks above: every shape opened below the
    // ceiling, plain was actually tried above it, and every long failure was a
    // length refusal rather than some other error.
    let plain_long_opened = plain_long.iter().any(|attempt| attempt.opened);
    let reparsing: Vec<&str> = long
        .iter()
        .filter(|attempt| !attempt.shape.survives_verbatim_parsing() && !attempt.opened)
        .map(|attempt| attempt.shape.label())
        .collect();

    // Whether the opt-in was actually in effect, which is the one input every
    // guard above ignores and the only one that decides what a refusal *means*.
    //
    // Both halves have to hold. With neither, or with only one, a refusal past
    // the ceiling is the documented outcome rather than a counter-example to it:
    // `MAX_PATH` applying to a process that has not opted in is what `MAX_PATH`
    // is. Reading it as "the documented reading is wrong" was this report's
    // flagship line in `probe-long-path-unaware` on every correctly configured
    // host -- announcing that Microsoft's documentation is refuted by the run
    // that most exactly confirms it, and doing so from the half of the pair whose
    // entire job is to be the baseline the other half is read against.
    // `None` for the registry cannot reach here -- a refused run returns from
    // `body` at the apparatus-error block above -- but it is spelled out rather
    // than lumped in with `Some(false)`, because "not consulted" is not "off" and
    // conflating them is the defect this third state was introduced to remove.
    let missing = match (observation.manifest_aware, observation.registry_enabled) {
        (false, Some(false)) => Some("neither half of the opt-in was in effect"),
        (false, Some(true)) => Some("this binary carries no `longPathAware` manifest"),
        (true, Some(false)) => Some("the machine's `LongPathsEnabled` is unset or 0"),
        (_, None) => Some("the machine's `LongPathsEnabled` was never read"),
        (true, Some(true)) => None,
    };

    // Both directions have to consult it, not just the refusal. "Lifted" credited
    // to an opt-in that was not in effect would be the same error facing the other
    // way, and a ceiling that lifts without opting in is a far stranger result
    // than either verdict below describes.
    if let Some(missing) = missing {
        if plain_long_opened {
            return format!(
                "-- UNEXPECTED, and not a verdict on the opt-in. A relative path resolved\n   \
                 past the ceiling while the opt-in was NOT in effect:\n   \
                 {missing}.\n   \
                 Nothing here credits the opt-in with that, because it was not on.\n   \
                 Establish why this host lifts the ceiling unopted before reading any\n   \
                 comparison against it."
            );
        }
        return format!(
            "-- BASELINE, not a finding. `MAX_PATH` applied to a relative path past the\n   \
             ceiling, which is what it does when the opt-in is not in effect:\n   \
             {missing}.\n   \
             This is the case the opted-in half is read against; it neither supports\n   \
             nor contradicts the documented reading."
        );
    }

    if !plain_long_opened {
        return "-- MAX_PATH was NOT lifted for a relative path: the opt-in was in effect --\n   \
                manifest and registry both -- and even the plain shape was refused past\n   \
                the ceiling. The documented reading is wrong for this configuration."
            .to_string();
    }
    if reparsing.is_empty() {
        "-- MAX_PATH was lifted for a relative path, and the path was still parsed\n\
            normally: `..` and forward slashes resolved past the ceiling exactly as\n\
            they do below it. No evidence of a prefix-then-parse implementation."
            .to_string()
    } else {
        format!(
            "-- SHARP EDGE. Length was lifted, but these shapes stopped resolving past\n\
                the ceiling while working below it: {}.\n\
                That is the signature of regularize-then-prefix: `\\\\?\\` disables exactly\n\
                these features, so a relative path changes meaning at MAX_PATH.",
            reparsing.join(", ")
        )
    }
}
