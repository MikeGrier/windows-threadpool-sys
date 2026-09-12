// Copyright (c) Mike Grier.

//! Correspondences a rendered report must satisfy *between* its parts.
//!
//! # Why this exists rather than more per-part tests
//!
//! A pull-request review found [`crate::topology_report`] printing
//! `BUG IN THIS PROBE ... Nothing below about cache partitioning can be
//! trusted` while the verdict two paragraphs below read `=> agree`. Twenty-eight
//! rounds of per-artifact review and a zero-surviving-mutant `cargo-mutants` run
//! had both passed over it, because **every function involved was correct on its
//! own terms** and the defect lived in the relation between two of them.
//!
//! That is the shape no per-part instrument can see. A test asserts one
//! function's output; a mutant perturbs one function's behaviour; a reviewer
//! reads one artifact and finds it locally true. A contradiction between two
//! locally-true parts is invisible to all three.
//!
//! # It reads the artifact, not the state that produced it
//!
//! Every check here works on the **rendered text** -- the thing a reader and a
//! log-mining pass actually receive. Checking internal state instead would miss
//! precisely the defect class this exists for: the state was consistent in the
//! case above, and the two renderings of it were not.
//!
//! # What it deliberately does not do
//!
//! It does not re-derive what the renderer should have printed. A second
//! implementation of the rendering rules would be a check of the copy rather
//! than of the contract, and would drift from the original the moment either
//! moved. Each rule below relates **two things already visible in the report**,
//! so the oracle has no opinion of its own to go stale.
//!
//! Over-constraining is the same defect as under-specifying, so a report that
//! omits a fact is not a violation -- absence is checked only where the report
//! itself makes a claim that requires the other part to agree.
//!
//! # Every correlation here is one the REPORT already renders twice
//!
//! That is the admission rule. A correspondence is added when the report states
//! the same fact in both halves and nothing relates the two -- never because
//! someone imagined that two things ought to correspond. The test is a property
//! of the artifact, not of anyone's intuition about it.
//!
//! **Two ways of finding one, and the rule admits both.** Most were caught the
//! hard way, by a rendered report contradicting itself; each variant below names
//! the contradiction that earned it. The structured pairs in
//! `check_structured_pairs` (private, so named rather than linked) were found
//! the systematic way instead, by walking
//! every NDJSON field against the prose and seeing which facts were rendered
//! twice with nothing comparing them -- no defect had occurred, and waiting for
//! one would have been the worse plan.
//!
//! This section said "every correlation here was observed violated", which
//! excluded the second route and so contradicted that function's own history
//! four screens below. Written today, while correcting a different overstatement
//! in the same paragraph; a rule stated more strongly than the code supports is
//! the exact defect this module exists to catch, and it went in as part of the
//! fix for one. Found by a review.
//!
//! **Stated as a rule rather than a count, deliberately.** This paragraph used
//! to read "seeded with three correlations ... a fourth is added when a fourth
//! contradiction is found" -- while the enum directly below it already had four
//! variants. The census was wrong when it was written, survived every review of
//! this branch, and would have gone stale again at the next addition even if it
//! had been right. The rule cannot: it stays true however many there are, and
//! it is the part a reader actually needs, since what matters is that nothing
//! here is speculative rather than how much of it there is.

/// A correspondence between two parts of a report that did not hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Correspondence {
    /// The prose raised an alarm while the verdict said everything matched.
    ///
    /// The original defect. An alarm is a statement that some part of the
    /// report cannot be trusted; a verdict of `agree` is a statement that every
    /// check made was made and matched. Both can be locally true and they
    /// cannot both describe the same run.
    AlarmWithAgreeingVerdict {
        /// The alarm line found in the prose.
        alarm: String,
        /// Where the agreeing verdict was found: `prose` or `ndjson`.
        verdict_source: &'static str,
    },
    /// One fact rendered twice, with the two renderings disagreeing.
    ///
    /// A report carries its findings for a human in prose and for a mining pass
    /// in NDJSON. A consumer that reconciles the two cannot, and neither
    /// rendering is self-evidently the wrong one.
    ProseAndNdjsonDisagree {
        /// What the fact is called, for the reader of the failure.
        fact: &'static str,
        /// As the prose rendered it.
        prose: String,
        /// As the NDJSON rendered it.
        ndjson: String,
    },
    /// A hardware claim was stated without its caveat while the report's own
    /// evidence says the parse was in doubt.
    ///
    /// The renderer's rule is that every hardware conclusion is gated on the
    /// parse being whole. A claim printed bare, in a report that elsewhere
    /// reports doubt, is that rule with an exception -- and a rule with an
    /// exception is not a rule.
    UncaveatedClaimUnderDoubt {
        /// The claim that was stated bare.
        claim: &'static str,
        /// The report's own visible evidence of doubt.
        evidence: String,
    },
    /// The banner names one machine and the body describes another.
    ///
    /// The banner is the line a reader uses to decide whether two runs are
    /// comparable at all, so a banner describing a different machine from the
    /// body under it invalidates every comparison drawn from the report --
    /// while each half stays locally correct, which is what let this survive
    /// review.
    BannerDisagreesWithBody {
        /// The processor count the banner named.
        banner: String,
        /// The processor count the body reported.
        body: String,
    },
    /// The prose states a fact the machine-readable line does not carry at all.
    ///
    /// **Absence where the report has already made the claim.** This module
    /// holds that a report omitting a fact is not a violation -- but that rule
    /// is about facts the report never mentions. Once the PROSE states one, the
    /// report has made a claim that the other rendering is required to agree
    /// with, and a missing counterpart is that requirement going unmet rather
    /// than the fact being absent.
    ///
    /// Every comparison here was written as "both sides present, do they
    /// match", so a rendering that DROPPED a field read as silence: measured,
    /// deleting `"processors"` from the row, deleting the whole `policies`
    /// object, and deleting the partitioning discriminator each left a report
    /// the oracle accepted, with the prose still making all three claims. A
    /// mining pass reading such a row gets no value and no warning.
    ///
    /// `report_unmeasured` needs no exemption: it renders neither side of any
    /// TOPOLOGY fact, so there is no prose claim for a missing counter, cache or
    /// policy to leave unanswered.
    ///
    /// Not "renders neither side" flatly, which is what this said and is false:
    /// the short object publishes `arch`, and the banner can name the same
    /// architecture, so that one correspondence IS rendered twice on an
    /// unmeasured report -- and is checked there, by
    /// `an_architecture_contradiction_survives_an_attribution_disclaimer`. The
    /// exemption this paragraph explains is about the facts the short object
    /// omits, not about the whole shape.
    RenderedOnlyInProse {
        /// The fact the prose stated.
        fact: &'static str,
        /// What the prose said, with nothing to compare it against.
        prose: String,
    },
    /// The verdict says every check was made, and a check's evidence is absent.
    ///
    /// **An `agree` verdict is a claim about what the run DID, not only about
    /// what matched.** `CrossCheck` reports `agree` to mean every check this
    /// probe could make was made and matched -- so a report that agrees while
    /// omitting the line a check reads is contradicting its own verdict, even
    /// though the two halves it still renders agree perfectly.
    ///
    /// Measured: deleting the `GetActiveProcessorCount` line from an otherwise
    /// untouched agreeing report left `check` returning nothing at all. The
    /// counter comparison simply skipped, because it was written to compare two
    /// present values and to say nothing otherwise -- the same shape as the
    /// dropped-counterpart class, in the one place where the VERDICT is what the
    /// missing side contradicts.
    EvidenceMissingWithAgreeingVerdict {
        /// The correspondence whose rendering the report did not carry.
        fact: &'static str,
    },
}

/// Every correspondence `report` violates, in the order they were checked.
///
/// An empty result means every correlation this oracle knows about held. It
/// does **not** mean the report is correct: an oracle is a floor, not a
/// specification.
#[must_use]
pub fn check(report: &str) -> Vec<Correspondence> {
    let mut found = Vec::new();
    let ndjson = ndjson_line(report);

    check_alarm_against_verdict(report, ndjson, &mut found);
    check_prose_against_ndjson(report, ndjson, &mut found);
    check_claims_against_doubt(report, ndjson, &mut found);
    check_structured_pairs(report, ndjson, &mut found);
    check_counters_against_verdict(report, ndjson, &mut found);
    check_banner_against_body(report, ndjson, &mut found);
    check_diagnostics_against_verdict(report, ndjson, &mut found);
    check_partitioning_answer(report, ndjson, &mut found);

    found
}

/// Each arm of the partitioning discriminator, and the prose that announces it.
///
/// **Mapped from the renderer, arm by arm, rather than guessed.** A rule written
/// against a subset produces false violations on the arms it guessed wrong, and
/// that failure mode is not hypothetical here: this same module once shipped a
/// banner rule asserting a correspondence `attribution()` explicitly declines to
/// claim, and it had to be narrowed after a review reproduced the false positive.
///
/// The markers are each arm's OPENING sentence, which is the part that cannot be
/// confused with another arm's. Changing any of these strings is a change to the
/// report's contract with its readers, not a rewording.
const PARTITIONING_ARMS: &[(&str, &str)] = &[
    (
        "outermost cache that partitions the processors it covers: ",
        "level",
    ),
    ("no cache level reported more than one domain", "none"),
    ("no cache levels were reported at all", "no_levels_reported"),
    (
        "at least one cache level reported more than one distinct domain",
        "not_unique",
    ),
    (SUMMARY_MISSING_MARKER, "summary_missing"),
];

/// The partitioning answer the prose gives, against the one the NDJSON publishes.
///
/// Gap 2 of M2.10, found by a review corrupting a real report and watching the
/// oracle accept it. The oracle already compared
/// `outermost_partitioning_cache_level` -- the NUMBER -- so the level was
/// checked. What went unread was the DISCRIMINATOR, the field saying whether a
/// level was selected at all: changing `"level"` to another value left the
/// oracle silent while the prose still read `outermost cache that partitions the
/// processors it covers: L2 (8 domains)`.
///
/// The two are opposite answers to this probe's central question -- "a level
/// partitions" against "none does" -- and the renderer is explicit about why the
/// field exists: the level alone said `null` for every absent case alike, so a
/// query counting nulls as "machines no cache level partitions" folded in
/// machines where a level DOES partition. This rule is what stops the two
/// renderings of that answer drifting apart.
fn check_partitioning_answer(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let Some(ndjson) = ndjson else {
        return;
    };
    let announced: Vec<&str> = PARTITIONING_ARMS
        .iter()
        .filter(|(marker, _)| has_line_beginning(report, marker))
        .map(|(_, arm)| *arm)
        .collect();

    // **Absent WITH a prose arm is a dropped discriminator, not silence.**
    // `report_unmeasured` carries neither side and so announces no arm, which is
    // why the exemption needed no special case -- but returning early on the
    // field alone also excused a MEASURED report that printed an arm and lost
    // its discriminator. Measured: deleting the field from a `level` report left
    // the prose conclusion standing with nothing to relate it to, and the oracle
    // accepted it.
    let Some(published) = ndjson_field(ndjson, "outermost_partitioning_cache") else {
        // ANY announced arm, not just a single one. Written for the single-arm
        // slice first, which excused the worse report of the two: two prose
        // answers AND no discriminator.
        if !announced.is_empty() {
            found.push(Correspondence::RenderedOnlyInProse {
                fact: "outermost partitioning answer",
                prose: announced.join(" and "),
            });
        }
        return;
    };

    match announced.as_slice() {
        // The prose names no partitioning answer at all. Not a contradiction --
        // the rule fires only where the report makes the claim twice.
        [] => {}
        [announced] if *announced == published => {}
        [announced] => found.push(Correspondence::ProseAndNdjsonDisagree {
            fact: "outermost partitioning answer",
            prose: (*announced).to_owned(),
            ndjson: published.to_owned(),
        }),
        // Two arms' prose in one report. The arms are exclusive by construction
        // -- they are one `match` -- so this is the prose giving two answers to a
        // question that has one, whatever the NDJSON says.
        several => found.push(Correspondence::ProseAndNdjsonDisagree {
            fact: "outermost partitioning answer",
            prose: several.join(" and "),
            ndjson: published.to_owned(),
        }),
    }

    // **`summary_missing` names a LEVEL in both renderings, and it was the one
    // arm whose number nothing compared.** Found by a review, after the rule
    // above had closed the discriminator itself -- which is the pattern this
    // module keeps repeating: a rule is added per fact, so the fact added
    // alongside it goes unread.
    //
    // The oracle's other level comparison is keyed to the `Level` arm's prose
    // label, `outermost cache that partitions the processors it covers: `, so it
    // never fires here. And `summary_missing` is the only non-`Level` arm whose
    // NDJSON level is a NUMBER rather than `null`, which is exactly what made
    // the omission invisible: the three arms beside it have no number to
    // disagree about.
    //
    // It matters most precisely where it was missing. This arm is the state the
    // renderer prints as `BUG IN THIS PROBE ... Nothing below about cache
    // partitioning can be trusted` -- a report already telling its reader it is
    // unreliable, in which the two renderings of WHICH level went unchecked.
    if let Some(prose_level) = summary_missing_level(report) {
        compare(
            found,
            "summary-missing outermost level",
            prose_level,
            ndjson_field(ndjson, "outermost_partitioning_cache_level"),
        );
    }
}

/// The level the `summary_missing` prose names, if the report carries that arm.
///
/// Keyed to the same opening sentence [`PARTITIONING_ARMS`] uses, so the two
/// cannot drift apart: if that sentence is reworded, both stop matching together
/// rather than one silently continuing to match a report the other no longer
/// recognises.
fn summary_missing_level(report: &str) -> Option<&str> {
    leading_digits(after_marker_leading_a_line(report, SUMMARY_MISSING_MARKER)?)
}

/// The text following `marker` on a line the RENDERER leads with it.
///
/// **A marker search over the whole report reads the caller's text as the
/// probe's.** The banner is contained into the first line rather than dropped,
/// so its content survives -- and an unanchored `find` then picks a marker out
/// of it wherever it lands. Measured before this: a banner ending
/// `... named L99 as the outermost` produced a `summary-missing outermost level`
/// of 99 against the row's 1, and one ending
/// `windows-topology-sys recorded 99 enumeration anomalies` produced an anomaly
/// count of 99 against the row's 0. Both are the oracle raising a violation
/// about a report that does not contain the defect -- a false alarm invented out
/// of caller text, which is the failure mode that costs a reader the most.
///
/// The rest of the module reads by line start, and these two were what was left
/// of the older style. The bullet is part of the renderer's shape, not a
/// concession: `CrossCheck` writes its diagnostic entries as `    - <sentence>`,
/// so the marker genuinely never begins its line. Accepting the bullet cannot
/// re-open the hole it closes, because a contained banner is one line beginning
/// `host:` and an attribution-shaped one is `host:` lines and a disclaimer --
/// neither can present a line whose first content is `- `.
fn after_marker_leading_a_line<'a>(report: &'a str, marker: &str) -> Option<&'a str> {
    report
        .lines()
        .find_map(|line| strip_entry_tag(line.trim_start()).strip_prefix(marker))
}

/// A diagnostic entry's leading tag, removed.
///
/// **`CrossCheck`'s entries carry a tag whose spelling depends on the verdict,
/// and reading only one of them left the rule unread on the others.** The
/// `INCOMPLETE` arm writes `    - {caveat}`; the `DISAGREE` arm writes
/// `    (parse incomplete) {caveat}` and `    (not compared) {skipped}` so a
/// reader can tell the disagreement from what was merely not established.
/// Anchoring to the bullet alone therefore went blind to the anomaly count on
/// exactly the verdict where a reader most needs it. Measured: the
/// `(parse incomplete) ` rendering returned `None` where the `- ` rendering
/// returned `Some("2")`, so corrupting the count on a disagreeing report raised
/// no violation at all.
///
/// Matches the SHAPE of a tag rather than restating the renderer's two literal
/// strings, which would be a second copy to drift. Containment makes that safe:
/// a contained banner is one line beginning `host:` and an attribution-shaped
/// one is `host:` lines and a disclaimer, so neither can present a line whose
/// first content is a bullet or a parenthesised tag.
fn strip_entry_tag(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix("- ") {
        return rest;
    }

    line.strip_prefix('(')
        .and_then(|rest| rest.split_once(") "))
        .map_or(line, |(_tag, rest)| rest)
}

/// The run of ASCII digits `text` opens with, if it opens with one.
fn leading_digits(text: &str) -> Option<&str> {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());

    (end > 0).then(|| &text[..end])
}

/// The opening sentence of the `summary_missing` arm, up to the level it names.
const SUMMARY_MISSING_MARKER: &str = "BUG IN THIS PROBE: the topology crate named L";

/// The diagnostic counts, against the verdict drawn beside them and against the
/// prose that lists the same entries.
///
/// `not_compared`, `parse_incomplete` and `enumeration_anomalies` are published
/// in the NDJSON and their entries are listed in the prose, and until now
/// nothing compared any of the three. `parse_incomplete` was read only as a
/// nonzero predicate for the heterogeneity caveat, which is a different question
/// from whether the two renderings agree.
///
/// **The verdict rule binds to a SPECIFIED contract, not to current behaviour.**
/// [`crate::topology_report`] states it where the NDJSON is emitted: "anomalies
/// populate `parse_incomplete`, and a non-empty `parse_incomplete` forces the
/// verdict away from `agree`, so `cross_check == "agree"` IMPLIES no record
/// failed to decode", and the crate pins it with
/// `a_dropped_enumeration_record_blocks_agreement_even_when_every_counter_matches`
/// rather than leaving it a promise in a comment. So an `agree` verdict beside a
/// nonzero count of either is the report contradicting its own published rule --
/// the same shape as the alarm-beside-an-agreeing-verdict defect this module was
/// built for, in the field a mining pass trusts most.
///
/// The implication is stated ONE WAY and is read that way here: a run whose
/// counter failed to read has a complete parse and still reports `incomplete`,
/// so a nonzero `not_compared` is NOT asserted to force the verdict, and no rule
/// below claims it does.
fn check_diagnostics_against_verdict(
    report: &str,
    ndjson: Option<&str>,
    found: &mut Vec<Correspondence>,
) {
    let Some(ndjson) = ndjson else {
        return;
    };

    if ndjson_field(ndjson, "cross_check") == Some("agree") {
        // `not_compared` belongs here too, and was missing. `CrossCheck`'s
        // verdict makes `agree` imply that nothing was skipped as well as that
        // nothing failed to decode, so an agreeing report publishing skipped
        // work contradicts its own rule exactly as a nonzero `parse_incomplete`
        // does. Measured, before this: `"cross_check":"agree"` beside
        // `"not_compared":3` was accepted with no violation. Found by a review.
        for key in ["parse_incomplete", "enumeration_anomalies", "not_compared"] {
            if let Some(count) = ndjson_field(ndjson, key)
                && count != "0"
            {
                found.push(Correspondence::AlarmWithAgreeingVerdict {
                    alarm: format!("\"{key}\":{count}"),
                    verdict_source: "ndjson",
                });
            }
        }
    }

    // **The anomaly COUNT is inside the prose sentence, not inferable from the
    // line count.** `CrossCheck` emits one `parse_incomplete` entry reading
    // `windows-topology-sys recorded N enumeration anomal...` however many there
    // were, so counting lines can never check N -- a report can say it recorded
    // 2 while publishing 99 and agree about every total. Found by a review,
    // which also caught that the checklist claimed all three diagnostic counts
    // were compared "against the prose listings" when this one was only ever
    // checked as a nonzero predicate under an `agree` verdict.
    //
    // Read for EVERY verdict, deliberately. The `agree` rule above is about a
    // contradiction with the verdict; this is about the two renderings of one
    // number, which must agree whatever the verdict says.
    if let Some(prose_anomalies) = anomaly_count_in_prose(report) {
        compare(
            found,
            "enumeration anomaly count",
            prose_anomalies,
            ndjson_field(ndjson, "enumeration_anomalies"),
        );
    }

    // The prose lists these entries one per line, and how it marks them depends
    // on the verdict: the DISAGREE arm labels each kind, so the two are counted
    // separately, while the INCOMPLETE arm renders both as a bare `- `, which
    // makes only their total recoverable. Counting what the prose can actually
    // distinguish, rather than a number it does not render, is the whole habit
    // this module is built on.
    if has_line_beginning(report, "=> DISAGREE") {
        // **Zero lines is not a prose claim.** These compare a COUNT OF LINES
        // against a field, and the renderer emits no line when the count is
        // zero -- so routing them through `compare`'s absence path reported a
        // dropped counterpart for a prose that had said nothing, on a fixture
        // that was previously accepted. Absence matters only where the prose
        // actually listed entries.
        for (label, key, fact) in [
            ("     (not compared) ", "not_compared", "not compared count"),
            (
                "     (parse incomplete) ",
                "parse_incomplete",
                "parse incomplete count",
            ),
        ] {
            let listed = prose_lines_beginning(report, label);
            match ndjson_field(ndjson, key) {
                Some(json) => compare_counts(found, fact, listed, json.parse().unwrap_or_default()),
                None if listed > 0 => found.push(Correspondence::RenderedOnlyInProse {
                    fact,
                    prose: listed.to_string(),
                }),
                None => {}
            }
        }
    }

    if has_line_beginning(report, "=> INCOMPLETE") {
        let listed = prose_lines_beginning(report, "     - ");
        let (Some(skipped), Some(caveats)) = (
            ndjson_count(ndjson, "not_compared"),
            ndjson_count(ndjson, "parse_incomplete"),
        ) else {
            // **The prose has already listed the entries here.** This arm sums
            // two fields, so it was written to return unless BOTH are present --
            // and that made a row which dropped either one silent on exactly the
            // verdict whose reason those counts carry. Found by the deletion
            // sweep, on a corpus shape rather than on this host: the counts are
            // zero here, so the entries are absent and there is nothing to drop.
            if listed > 0 {
                found.push(Correspondence::RenderedOnlyInProse {
                    fact: "incomplete-verdict listing count",
                    prose: listed.to_string(),
                });
            }
            return;
        };
        compare_counts(
            found,
            "incomplete-verdict listing count",
            listed,
            skipped + caveats,
        );
    }
}

/// The anomaly count named inside the diagnostic sentence, if it is present.
///
/// Keyed to the sentence `CrossCheck` writes, and reading the number that
/// follows it. The count is rendered INSIDE one entry rather than as one entry
/// per anomaly, so nothing about the number is recoverable from counting lines.
fn anomaly_count_in_prose(report: &str) -> Option<&str> {
    const MARKER: &str = "windows-topology-sys recorded ";
    leading_digits(after_marker_leading_a_line(report, MARKER)?)
}

/// How many lines of `report` begin with `prefix`.
fn prose_lines_beginning(report: &str, prefix: &str) -> usize {
    report
        .lines()
        .filter(|line| line.starts_with(prefix))
        .count()
}

/// An NDJSON field read as a count, or `None` when it renders no number.
fn ndjson_count(ndjson: &str, key: &str) -> Option<usize> {
    ndjson_field(ndjson, key)?.parse().ok()
}

/// Push a disagreement between two counts of the same thing.
fn compare_counts(found: &mut Vec<Correspondence>, fact: &'static str, prose: usize, json: usize) {
    if prose != json {
        found.push(Correspondence::ProseAndNdjsonDisagree {
            fact,
            prose: prose.to_string(),
            ndjson: json.to_string(),
        });
    }
}
/// The banner names the machine the body describes.
///
/// A run makes three discoveries of the host -- one before the measurement,
/// `measure`'s own, and one after -- and the banner used to be built from an
/// endpoint, so it could name a different topology from the body beneath it
/// with nothing in the report saying so.
///
/// **The rule applies only where the report claims the correspondence**, and
/// that qualifier is load-bearing here. `attribution` takes the two readings
/// that bracket the measurement and renders the first as the banner; the body
/// comes from a `measure()` between them. When the two endpoints differ, or
/// either fails, the report says so in as many words -- "which of them names the
/// machine the body below describes was not established" -- and on such a report
/// the first banner line need not describe the body at all. Asserting a
/// contradiction there would be the oracle over-claiming exactly where the
/// renderer went to trouble not to, so it returns instead.
///
/// **On the origin branch this rule was unconditional, and that was correct
/// there**: `measure_observed` builds the banner from the body's own topology,
/// which makes the mismatch unrepresentable. That construction change is a
/// separate peel, so the ambiguity is live on this branch and the rule has to
/// respect it. When the construction lands, the guard below stops being reached
/// rather than becoming wrong.
///
/// What the rule still buys where attribution IS determinate: the banner and the
/// body remain two independent *derivations* from one topology --
/// `Fingerprint::from_topology` and `observe`, each with its own filter for
/// which processors count. Those have already disagreed once, when
/// `from_topology` summed core-domain membership and printed `0p` for a machine
/// about to be measured on four processors.
fn check_banner_against_body(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let Some(ndjson) = ndjson else {
        return;
    };

    // The architecture is checked FIRST and outside the attribution exemption,
    // for the reason given on the function below.
    check_architecture_against_body(report, ndjson, found);

    // The report's own statement that it cannot attribute the body to either
    // reading. Matched on the rendered disclaimers rather than on the count of
    // `host:` lines, because the count is incidental to how `attribution`
    // happens to render today and these sentences are the contract.
    //
    // It exempts the PROCESSOR COUNT specifically, which is what the disclaimer
    // is about: the two readings name different topologies and this run cannot
    // say which describes the body.
    if has_line_beginning(report, "HOST READINGS DISAGREE:")
        || has_line_beginning(report, "HOST NOT ESTABLISHED:")
    {
        return;
    }

    let Some(banner) = report.lines().find(|line| line.starts_with("host:")) else {
        return;
    };

    // Absent on a report whose discovery failed: the banner reads `UNKNOWN` and
    // `report_unmeasured` emits no processor count, so there is nothing to
    // relate and no violation to claim.
    let (Some(banner_count), Some(body_count)) = (
        processors_in_banner(banner),
        ndjson_field(ndjson, "processors"),
    ) else {
        return;
    };

    if banner_count != body_count {
        found.push(Correspondence::BannerDisagreesWithBody {
            banner: banner_count.to_owned(),
            body: body_count.to_owned(),
        });
    }
}

/// The architecture every banner names, against the one the body publishes.
///
/// The architecture is rendered twice -- in the banner and as the NDJSON's
/// `arch` -- and went unread until a review corrupted one and watched the oracle
/// accept it. Both come from `std::env::consts::ARCH` today, so they cannot
/// currently differ; that is a fact about the renderer rather than a contract,
/// and exactly the kind of coincidence this oracle is built not to lean on.
///
/// **Outside the attribution exemption, and that is the point of separating it.**
/// The disclaimers say which of the two bracket READINGS describes the body was
/// not established. That is a statement about the machine's topology, not about
/// its instruction set: when every banner the report managed to parse names the
/// same architecture, then whichever reading describes the body, the
/// architecture is that one -- so a body naming a different one is a
/// contradiction the disclaimer does not excuse. Found by a review; before this,
/// two `x86_64` banners under a disclaimer beside an `aarch64` body produced no
/// violation at all.
///
/// **When the banners disagree with EACH OTHER about the architecture, this
/// returns**, because then the question really is unestablished and asserting
/// anything would be the over-claim the exemption exists to prevent.
///
/// Checked before the processor-count guard as well, so it still reaches
/// `report_unmeasured`, which emits `arch` but no `processors`.
fn check_architecture_against_body(report: &str, ndjson: &str, found: &mut Vec<Correspondence>) {
    let announced: Vec<&str> = report
        .lines()
        .filter(|line| line.starts_with("host:"))
        .filter_map(architecture_in_banner)
        .collect();

    let Some(first) = announced.first() else {
        return;
    };
    if announced.iter().any(|architecture| architecture != first) {
        return;
    }

    // Past the two returns above, the banner HAS named an architecture and every
    // reading agrees on it -- so a row without `arch` is the report stating the
    // fact once, not a report that never stated it.
    compare(
        found,
        "architecture",
        first,
        ndjson_field(ndjson, "arch").map(|body| body.trim_matches('"')),
    );
}
/// The architecture the banner names.
///
/// The fingerprint renders as `<arch> <N>p/<M>c ...`, optionally behind one or
/// more `!!MARKER!!` prefixes, so the architecture is the first token that is
/// not one of those.
///
/// **Every marker is skipped, not a named list of them.** The first version of
/// this stripped `!!taint!!` alone and read `!!assumed!!` as an architecture,
/// reporting a contradiction against a perfectly good banner -- caught by an
/// existing acceptance test, which is what those are for.
///
/// A first attempt at justifying that generality said "the renderers emit at
/// least `!!SYNTHETIC!!`, `!!UNOFFICIAL!!`, `!!RESTORED!!`, `!!assumed!!` and
/// `!!taint!!`", which is an overclaim of the kind this module exists to catch,
/// written while fixing another one. Counted: a `host:` line is rendered by
/// `banner_line`/`banner_line_for` from a `Fingerprint`, which emits
/// `!!{provenance}!!` only when the provenance is not `Measured` -- so the only
/// markers this function can meet today are **`!!SYNTHETIC!!` and
/// `!!RESTORED!!`**. `!!UNOFFICIAL!!` belongs to `BuildIdentity`, which never
/// reaches this line, and `!!assumed!!` and `!!taint!!` are not emitted anywhere
/// -- they exist only as invented fixtures in this module's own tests.
///
/// The generality is still right, and on a better argument than a miscounted
/// list: the marker is a *shape* the banner reserves for provenance, and this
/// module observes the renderer rather than mirroring it. Matching the shape
/// cannot fall out of step; enumerating today's two spellings would.
///
/// **Only a FINGERPRINT names an architecture, so the count shape is required
/// here.** A failed read names no architecture, and a caller may pass any string
/// as a banner. Without that guard the first token of such a line is read as an
/// architecture and contradicts the NDJSON every time: measured, `UNKNOWN`
/// against a real `"arch"` produced a false violation, and the crate's own
/// `host:  TEST-FIXTURE` fixture produced fourteen more. What the failed-read
/// line actually looks like -- and why the guard has to read by position rather
/// than by substring -- is on `fingerprint_tokens`, which owns that decision.
/// (Named without a link: it is private, and a public doc may not link to it.)
///
/// This paragraph used to describe that line as "the bare word `UNKNOWN`", which
/// is not what `banner_line_for` writes. The correction was made on
/// `fingerprint_tokens` and not here, so the two docs contradicted each other
/// in the same module -- a fix applied to one statement of a fact while another
/// statement of it survived, which is the drift this crate keeps paying for.
///
/// Note carefully which side this constrains. Requiring the BANNER to carry
/// `<N>p/<M>c` is what establishes it is a fingerprint; requiring the BODY to
/// carry a processor count is the coupling that wrongly confined this rule to
/// measured reports. The first is the renderer's contract, the second was an
/// accident of where the code sat.
/// **Public because the question has to have ONE answer.** A test helper asked
/// the same thing -- "does this `host:` line name an architecture?" -- with its
/// own cheaper rule, `line.contains("p/")`. That agreed with this module until
/// this module started reading the tokens by position, and then it did not:
/// measured, a failed-discovery banner of
/// `host:  UNKNOWN -- topology discovery failed: 16p/foo something opaque`
/// satisfied the helper and not the oracle, so the fact accounting demanded that
/// `arch` be read on a report where the oracle is right to say nothing, and a
/// perfectly valid unmeasured report failed the suite.
///
/// A second implementation of a predicate is not a check of it; it is a copy
/// that agrees until it does not. Consumers ask here instead.
pub fn architecture_in_banner(banner: &str) -> Option<&str> {
    fingerprint_tokens(banner).map(|(architecture, _)| architecture)
}

/// The `<arch>` and `<N>p/<M>c` tokens a banner names, if it names a fingerprint.
///
/// **Read BY POSITION, because a banner that is not a fingerprint can still
/// contain the substrings one would have.** Both readers used to search the
/// whole line -- the count as "digits before the first `p/`", the architecture
/// as "first token that is not a taint marker and does not contain `p/`" -- and
/// a banner only has to mention `p/` somewhere for that to find a fingerprint
/// in text that is not one.
///
/// That is not hypothetical, and the doc this replaces had the renderer's own
/// shape wrong: `banner_line_for` does NOT render a failed read as the bare
/// word `UNKNOWN`. It renders
/// `host:  UNKNOWN -- topology discovery failed: {error}`, with the `io::Error`
/// verbatim. Measured: an error text of `16p/foo something opaque` made the
/// count reader answer `16`, which satisfied the guard, so the architecture
/// reader then answered `UNKNOWN` and the bound assertion PANICKED on a
/// perfectly valid unmeasured report -- a probe crashing on the host whose
/// discovery failed, which is the host it exists to report.
///
/// The fingerprint renders `<arch> <N>p/<M>c smt<S>` behind any number of
/// `!!taint!!` markers, so the two tokens are taken from their positions and
/// the second is required to have the count SHAPE. `UNKNOWN -- ...` fails that
/// on its second token and is silent, which is the right answer: a failed read
/// names no architecture, so there is nothing to compare.
///
/// Found together rather than separately because both readers need exactly the
/// same decision -- "is this a fingerprint, and where are its parts" -- and two
/// copies of that decision are what let them disagree about it before.
fn fingerprint_tokens(banner: &str) -> Option<(&str, &str)> {
    let mut tokens = banner
        .strip_prefix("host:")?
        .split_whitespace()
        .skip_while(|token| token.starts_with("!!") && token.ends_with("!!"));

    let architecture = tokens.next()?;
    let counts = tokens.next()?;

    is_count_shaped(counts).then_some((architecture, counts))
}

/// Whether `token` is the `<N>p/<M>c` the fingerprint writes.
fn is_count_shaped(token: &str) -> bool {
    let Some((processors, rest)) = token.split_once("p/") else {
        return false;
    };
    let Some(cores) = rest.strip_suffix('c') else {
        return false;
    };

    [processors, cores]
        .iter()
        .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The processor count a banner line names, as it was rendered.
///
/// Returned as text rather than parsed, so a malformed count is reported as the
/// mismatch it is instead of being silently discarded by a failed parse.
fn processors_in_banner(banner: &str) -> Option<&str> {
    let (_, counts) = fingerprint_tokens(banner)?;

    counts.split_once("p/").map(|(processors, _)| processors)
}

/// [`check`], as an assertion, for tests that render a report.
///
/// # Panics
///
/// Panics listing every correspondence the report violated.
pub fn assert_corresponds(report: &str) {
    let violations = check(report);
    assert!(
        violations.is_empty(),
        "the report's parts contradict each other: {violations:#?}\n\n\
         --- the report ---\n{report}"
    );
}

/// The report's machine-readable line, if it has one.
///
/// A report is prose with at most one NDJSON line in it. `report_unmeasured`
/// emits a much shorter object than `report`, so every field read below is
/// optional by construction.
fn ndjson_line(report: &str) -> Option<&str> {
    report.lines().find(|line| line.starts_with('{'))
}

/// The raw text of one field of a flat JSON object.
///
/// Hand-written rather than pulled from a JSON crate because this crate has no
/// such dependency and the object is emitted a few lines away in this same
/// crate: a top-level object of machine-generated fields, whose only nesting is
/// the `caches` array of objects and the `policies` object, both of which the
/// balanced scan below handles. An earlier version of this sentence called the
/// object "flat, unnested", which would lead a future change to assume nested
/// values are unsupported when they are read here every run. It returns the value's source
/// text -- quotes stripped for a string, otherwise verbatim -- so a caller
/// compares renderings rather than parsed values, which is the point.
fn ndjson_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];

    let value = if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        &stripped[..end]
    } else if rest.starts_with('[') || rest.starts_with('{') {
        // Balanced, not first-closer. `caches` is an array OF objects and
        // `policies` is an object, so stopping at the first `]` or `}` would
        // truncate both -- returning `[{"level":1,"domains":8` for a three-level
        // machine, which then compares unequal against anything and reports a
        // contradiction that is the reader's own parse.
        let end = balanced_end(rest)?;
        &rest[1..end]
    } else {
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        &rest[..end]
    };

    Some(value.trim())
}

/// One field of a flat JSON object, with its delimiters left on.
///
/// [`ndjson_field`] strips the quotes from a string and the brackets from an
/// array or object, which is what most callers want -- they are comparing
/// contents. A caller that cares whether the value IS an array needs the
/// delimiter, because a scalar and a one-element list have the same contents.
fn ndjson_raw_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];

    let end = if rest.starts_with('[') || rest.starts_with('{') {
        balanced_end(rest)? + 1
    } else if let Some(after_quote) = rest.strip_prefix('"') {
        after_quote.find('"')? + 2
    } else {
        rest.find([',', '}']).unwrap_or(rest.len())
    };

    Some(rest[..end].trim())
}

/// The index of the bracket closing the one `text` opens with.
fn balanced_end(text: &str) -> Option<usize> {
    let mut depth = 0_i32;
    for (index, character) in text.char_indices() {
        match character {
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether some line of the report BEGINS with this renderer token.
///
/// **Anchored, because a report carries text the renderer does not own.**
/// `report_unmeasured` embeds the caller's `io::Error`, so an unanchored
/// substring search reads that error as if it were the probe speaking. Measured,
/// before this: an error reading `BUG IN THIS PROBE => agree` made a perfectly
/// valid unmeasured report trip the alarm rule and panic in the renderer's own
/// binding -- the oracle inventing a contradiction out of a message it should
/// have treated as opaque. Found by a review.
///
/// **The renderer now contains that text, and this anchoring is still what
/// stops it being read.** `renderer_owns_every_line` flattens the error, so it
/// can no longer introduce a LINE -- but its words still sit inside the
/// discovery-failure line, and an unanchored search finds them there just the
/// same. The two fixes answer different halves: containment stops caller text
/// impersonating a line, anchoring stops it being read as one. This paragraph
/// said "prints verbatim", which described the state before containment and
/// made the anchoring look redundant.
///
/// Leading whitespace is trimmed rather than matched, because the renderer
/// indents some of these lines and not others, and the indentation is
/// presentation rather than contract. What matters is that the token begins a
/// line: every token passed here is one the renderer writes at the start of a
/// line it owns, and the caller's error is embedded mid-line after
/// `MachineMemoryTopology::discover failed: `.
///
/// **The residue, stated rather than implied, and narrower than it was.** An
/// error containing a newline followed by one of these tokens would once have
/// put that token at the start of its own line, where anchoring cannot help. The
/// renderer closed that: `report_unmeasured` passes the error through
/// `renderer_owns_every_line`, so no error can create a line any more. Measured:
/// an error of `first\nBUG IN THIS PROBE: second` produces no line beginning
/// with the alarm, and the report is accepted.
///
/// What remains is only for a caller that hands `check` text the renderer never
/// produced -- a test constructing a report by hand, say. Bounding even that
/// needs the renderer to tell the oracle which region is opaque, which is a
/// change to the report format rather than to this reader.
fn has_line_beginning(report: &str, token: &str) -> bool {
    report
        .lines()
        .any(|line| line.trim_start().starts_with(token))
}

/// The claim's line and the indented continuation beneath it.
///
/// A gated claim and the caveat that excuses it are one rendered block: the
/// renderer writes the claim, then the caveat as an indented continuation, then
/// a blank line. Ending at that blank line is what keeps another block's caveat
/// from answering for this one.
///
/// **This also subsumes the caller-text problem, which is why the wider
/// `renderer_prose` filter it replaced is gone.** The caveat is matched
/// mid-line, so it is the one search here that cannot anchor to a line start --
/// and that is the SUPPRESSING direction: a caveat found where none was written
/// removes a violation. Measured, before any of this: with the caveat sentence
/// appended to the banner, `UncaveatedClaimUnderDoubt` disappeared from a report
/// that still carried the claim and still said `parse_incomplete=2`.
///
/// A block cannot start in the banner, because the claim line is found by its
/// own opening text and containment guarantees a caller's banner is one line
/// beginning `host:`. So scoping to the block excludes caller text for a
/// structural reason rather than by listing the places caller text can appear,
/// which is what the previous filter had to do -- and got wrong once, by
/// anchoring to a title that `clean_report()` does not use.
/// `a_caveat_in_the_banner_does_not_excuse_an_uncaveated_claim` still pins it.
fn claim_block<'a>(report: &'a str, claim: &str) -> impl Iterator<Item = &'a str> {
    report
        .lines()
        .skip_while(move |line| !line.trim_start().starts_with(claim))
        .take_while(|line| !line.trim().is_empty())
}

/// The text after `label` on the line that begins with it.
fn prose_field<'a>(report: &'a str, label: &str) -> Option<&'a str> {
    report
        .lines()
        .find(|line| line.starts_with(label))
        .map(|line| line[label.len()..].trim())
}

/// Alarms the prose can raise. Each is a statement that part of the report is
/// not to be trusted.
const ALARMS: &[&str] = &["BUG IN THIS PROBE"];

fn check_alarm_against_verdict(
    report: &str,
    ndjson: Option<&str>,
    found: &mut Vec<Correspondence>,
) {
    let Some(alarm) = report.lines().find(|line| {
        ALARMS
            .iter()
            .any(|marker| line.trim_start().starts_with(marker))
    }) else {
        return;
    };

    if has_line_beginning(report, "=> agree") {
        found.push(Correspondence::AlarmWithAgreeingVerdict {
            alarm: alarm.trim().to_owned(),
            verdict_source: "prose",
        });
    }

    if ndjson.and_then(|line| ndjson_field(line, "cross_check")) == Some("agree") {
        found.push(Correspondence::AlarmWithAgreeingVerdict {
            alarm: alarm.trim().to_owned(),
            verdict_source: "ndjson",
        });
    }
}

/// Facts this report renders twice: the prose label, the NDJSON key, and the
/// name to use when they disagree.
///
/// Counts only. A prose line reads `processors (online) : 16` and the NDJSON
/// `"processors":16`, so the comparison is of the rendered values with the
/// prose label removed.
const DOUBLE_RENDERED: &[(&str, &str, &str)] = &[
    ("processors (online) : ", "processors", "online processors"),
    ("processor groups    : ", "groups", "processor groups"),
    ("packages            : ", "packages", "packages"),
    ("physical cores      : ", "cores", "physical cores"),
];

fn check_prose_against_ndjson(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let Some(ndjson) = ndjson else {
        return;
    };

    for (label, key, fact) in DOUBLE_RENDERED {
        let Some(prose) = prose_field(report, label) else {
            continue;
        };
        let Some(json) = ndjson_field(ndjson, key) else {
            // The prose made the claim; the row is required to carry it.
            found.push(Correspondence::RenderedOnlyInProse {
                fact,
                prose: prose.to_owned(),
            });
            continue;
        };
        if prose != json {
            found.push(Correspondence::ProseAndNdjsonDisagree {
                fact,
                prose: prose.to_owned(),
                ndjson: json.to_owned(),
            });
        }
    }

    // The efficiency classes, whose two renderings differ in punctuation and so
    // cannot be compared as text. This pair is here because it was wrong: the
    // NDJSON once emitted the class COUNT under a plural name, so a
    // single-class host printed `"efficiency_classes":1` beside a prose
    // `efficiency classes: [0]` -- the same fact, in one report, in two
    // renderings a consumer cannot reconcile.
    // Gated on the PROSE, so a row that lost the field is a dropped counterpart
    // rather than silence -- the same rule `compare` applies, restated here only
    // because this pair needs RAW values and so cannot route through it.
    let classes_prose = prose_field(report, "  efficiency classes: ");
    let classes_json = ndjson_raw_field(ndjson, "efficiency_classes");

    if let Some(prose) = classes_prose
        && classes_json.is_none()
    {
        found.push(Correspondence::RenderedOnlyInProse {
            fact: "efficiency classes",
            prose: prose.to_owned(),
        });
    }

    if let (Some(prose), Some(json)) = (classes_prose, classes_json) {
        // **The container is part of the fact, and comparing only the contents
        // threw it away.** `normalise_list` strips `[` and `]` from both sides,
        // so a scalar `1` and a list `[1]` normalise to the same `"1"` -- which
        // means the very regression this pair exists for, a class COUNT emitted
        // under a plural name, survives undetected on any host whose single
        // class is `1`. The old test caught the historical case only because it
        // used class `[0]` against a count of `1`, so the VALUES differed; it
        // established nothing about the shape. Found by a review.
        //
        // A host with a single class `1` is a supported shape, not a contrived
        // one, so this is a live hole rather than a theoretical one.
        // **Both sides, because the first version of this checked one.** It
        // required the NDJSON to be a list and said nothing about the prose, so
        // the mirror drift -- prose falling to `efficiency classes: 1` while the
        // NDJSON still renders `[1]` -- was accepted: `normalise_list` strips
        // the brackets from the JSON side and the two compare equal. Measured,
        // before this: `check()` returned no violation for exactly that report.
        // Found by a review, in the fix for the other direction.
        if prose.starts_with('[') != json.starts_with('[') {
            found.push(Correspondence::ProseAndNdjsonDisagree {
                fact: "efficiency classes",
                prose: prose.to_owned(),
                ndjson: json.to_owned(),
            });
        } else {
            let prose_classes = normalise_list(prose);
            let json_classes = normalise_list(json);
            if prose_classes != json_classes {
                found.push(Correspondence::ProseAndNdjsonDisagree {
                    fact: "efficiency classes",
                    prose: prose_classes,
                    ndjson: json_classes,
                });
            }
        }
    }

    // The verdict, which the prose states as a sentence and the NDJSON as a
    // token.
    let prose_verdict = if has_line_beginning(report, "=> agree") {
        Some("agree")
    } else if has_line_beginning(report, "=> DISAGREE") {
        Some("disagree")
    } else if has_line_beginning(report, "=> INCOMPLETE") {
        Some("incomplete")
    } else {
        None
    };

    // The verdict is the report's central claim, so a row that lost it is the
    // worst case of the dropped-counterpart class rather than an exception to
    // it: the prose still announces an answer and nothing machine-readable
    // carries it.
    if let Some(prose) = prose_verdict {
        compare(
            found,
            "cross-check verdict",
            prose,
            ndjson_field(ndjson, "cross_check"),
        );
    }
}

/// The facts whose two renderings differ in shape rather than punctuation.
///
/// Found by the M2.4 matrix rather than by a defect. The four counts already
/// checked above were the ones a reviewer had happened to look at; walking every
/// NDJSON field against the prose showed these carrying the same fact twice as
/// well, with nothing comparing them.
fn check_structured_pairs(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let Some(ndjson) = ndjson else {
        return;
    };

    // `NUMA domains        : 1 (0 with no processors)` against two fields.
    if let Some(prose) = prose_field(report, "NUMA domains        : ") {
        let total = prose.split_whitespace().next().unwrap_or_default();
        let without = prose
            .split_once('(')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .unwrap_or_default();

        compare(
            found,
            "NUMA domains",
            total,
            ndjson_field(ndjson, "numa_domains"),
        );
        compare(
            found,
            "NUMA domains without processors",
            without,
            ndjson_field(ndjson, "numa_domains_without_processors"),
        );
    }

    // `  (3 reported only by CPU Sets, never by the relationship walk:` against
    // the NDJSON's count of the same thing.
    //
    // **Rendered CONDITIONALLY**, and that is why it needs its own lookup rather
    // than joining the pair above: the renderer emits the prose line only when
    // the count is above zero, so on most hosts there is no line to find and
    // `prose_field` returns `None`. That is silence, not agreement -- the rule
    // fires only where the report actually makes the claim twice.
    //
    // Found by a review, and it is the third double-rendered fact this module
    // shipped without reading. The other two were `arch` and the
    // `outermost_partitioning_cache` discriminator, now read by
    // `check_partitioning_answer` with all five arms mapped. The lesson the
    // three share:
    // a rule is added per fact, so the set of facts is the thing that drifts,
    // and nothing here derives that set from the renderer.
    // **Selected by what the line SAYS, not by being the first `  (` line.**
    // `prose_field` takes the first line with the label, and `  (` is not a
    // label -- it is the opening of any parenthesised continuation. A report
    // with heterogeneous efficiency classes writes
    // `  (heterogeneous: ...` ABOVE this one, so on that shape the first match
    // was the wrong line, the `contains` guard below rejected it, and the
    // CPU-Sets count went unread with no sign that it had.
    //
    // The accounting test reports that as an unread fact rather than hiding it,
    // which is how it was found -- but only on a report carrying both, and this
    // host renders neither.
    if let Some(prose) = report
        .lines()
        .find(|line| line.starts_with("  (") && line.contains("reported only by CPU Sets"))
        .map(|line| line["  (".len()..].trim())
    {
        compare(
            found,
            "NUMA domains reported only by CPU Sets",
            prose.split_whitespace().next().unwrap_or_default(),
            ndjson_field(ndjson, "numa_domains_only_in_cpu_sets"),
        );
    }

    // `outermost cache that partitions the processors it covers: L2 (8 domains)`
    // against the level the NDJSON names. This pair is the one the original
    // defect lived next to: the prose can name a level the machine-readable
    // line does not.
    if let Some(prose) = prose_field(
        report,
        "outermost cache that partitions the processors it covers: ",
    ) {
        let level = prose
            .trim_start_matches('L')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        compare(
            found,
            "outermost partitioning cache level",
            level,
            ndjson_field(ndjson, "outermost_partitioning_cache_level"),
        );
    }

    // The policy table against the `policies` object. A policy's domain count is
    // what the whole report is for, so two renderings of it disagreeing would
    // mislead exactly the reader who came for the answer.
    //
    // **The NAMES are compared before the counts, because the name is a
    // double-rendered fact and not merely a lookup aid.** Locating the NDJSON
    // entry by the prose name and comparing only the value makes a failed lookup
    // silent: `find()` returns `None`, `compare` returns early, and renaming
    // `by-core` on one side alone -- or dropping the entry, or adding one the
    // prose never mentions -- is accepted. Found by a review.
    // **The prose section is what decides whether the pair is required, so the
    // CONTAINER's absence is checked the same way a member's is.** Gating the
    // whole block on the object existing made "the renderer dropped the entire
    // `policies` object" silent while the prose table still stood beside it --
    // the member-level tests covered an entry going missing from the object,
    // never the object going missing from the row. Measured: deleting it left a
    // report the oracle accepted.
    let policies_section = has_line_beginning(report, "domains each policy would produce:");
    let policies = ndjson_field(ndjson, "policies");

    if policies_section && policies.is_none() {
        found.push(Correspondence::RenderedOnlyInProse {
            fact: "policy names",
            prose: "domains each policy would produce:".to_owned(),
        });
    }

    if let Some(policies) = policies {
        // Gated on the prose SECTION, not on the rows parsing. The header is
        // what establishes that the report renders this set twice; requiring a
        // non-empty row list instead would make "the prose table lost all its
        // rows" look like silence rather than the disagreement it is.
        if policies_section {
            compare_membership(
                found,
                "policy names",
                &policy_rows(report)
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
                &object_keys(policies),
            );
        }

        for (name, count) in policy_rows(report) {
            let key = format!("\"{name}\":");
            let json = policies
                .find(&key)
                .map(|at| &policies[at + key.len()..])
                .map(|rest| {
                    let end = rest.find(',').unwrap_or(rest.len());
                    rest[..end].trim()
                });

            // **An entry the object does not carry is a MEMBERSHIP finding, and
            // `compare_membership` above has already reported it by name.**
            // Letting the per-entry rule report absence as well produced a
            // second, weaker violation for the same defect -- one that says a
            // count had no counterpart without saying which policy it belonged
            // to. Each rule reports its own concern once.
            if json.is_some() {
                compare(found, "policy domain count", &count, json);
            }
        }
    }

    // The cache table against the `caches` array, level by level -- and the set
    // of LEVELS first, for the reason given above. A cache moved from level 3 to
    // level 9 in one rendering only is the same silent-lookup defect: the prose
    // still reads `L3`, nothing matches it, and the report is accepted.
    // The same container rule as `policies`: the prose SECTION is what makes the
    // pair required, so losing the whole array is a dropped counterpart and not
    // silence.
    let caches_section = has_line_beginning(report, "caches:");
    let caches_field = ndjson_field(ndjson, "caches");

    if caches_section && caches_field.is_none() {
        found.push(Correspondence::RenderedOnlyInProse {
            fact: "cache levels",
            prose: "caches:".to_owned(),
        });
    }

    if let Some(caches) = caches_field {
        if caches_section {
            compare_membership(
                found,
                "cache levels",
                &cache_rows(report)
                    .into_iter()
                    .map(|(level, _)| level)
                    .collect::<Vec<_>>(),
                &cache_levels(caches),
            );
        }

        for (level, domains) in cache_rows(report) {
            // **The object is located by its `level` member, then read for its
            // `domains` member -- two independent steps, so member order and
            // spelling are separate questions.**
            //
            // (History, because the shape of the bug is the reason for the
            // shape of the code. This used to match one literal,
            // `"level":N,"domains":`, which silently required the two members to
            // be adjacent and in that order. Renaming, reordering or dropping
            // `domains` alone made the lookup miss, and the prose domain count
            // went uncompared while `cache_levels` still found every level and
            // reported membership as agreeing. Measured then: both
            // `{"level":1,"x-domains":8}` and `{"domains":8,"level":1}` were
            // accepted, where `{"level":1,"domains":9}` was caught. The tests
            // below now require the reordered object to stay readable and the
            // renamed one to be reported.)
            //
            // A level the array does not carry at all is `compare_membership`'s
            // finding above, reported by level number, so it is skipped here --
            // but an object that IS there and cannot answer is a dropped
            // counterpart, which is `compare`'s business.
            if let Some(object) = cache_object(caches, &level) {
                compare(
                    found,
                    "cache domain count",
                    &domains,
                    ndjson_field(object, "domains"),
                );
            }
        }
    }
}

/// Push a disagreement when two renderings of one SET differ.
///
/// Sorted before comparing, so a renderer free to emit its entries in a
/// different order from the prose is not accused of disagreeing about which
/// entries exist. Both sides get the same comparator, so the comparison stays
/// consistent whatever that order is.
fn compare_membership(
    found: &mut Vec<Correspondence>,
    fact: &'static str,
    prose: &[String],
    ndjson: &[String],
) {
    let mut prose_sorted = prose.to_vec();
    let mut ndjson_sorted = ndjson.to_vec();
    prose_sorted.sort();
    ndjson_sorted.sort();

    if prose_sorted != ndjson_sorted {
        found.push(Correspondence::ProseAndNdjsonDisagree {
            fact,
            prose: prose_sorted.join(", "),
            ndjson: ndjson_sorted.join(", "),
        });
    }
}

/// The keys of a flat JSON object, in the order it renders them.
///
/// A key is a quoted string followed immediately by `:`. `policies` is flat --
/// name to count -- so no nesting has to be tracked here, and a value that
/// happened to be a string could not be mistaken for a key because it is not
/// followed by a colon.
fn object_keys(object: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = object;

    while let Some(open) = rest.find('"') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('"') else {
            break;
        };
        let (key, tail) = after_open.split_at(close);
        let tail = &tail[1..];
        if tail.starts_with(':') {
            keys.push(key.to_owned());
        }
        rest = tail;
    }

    keys
}

/// The `level` of each entry of the `caches` array, as rendered.
fn cache_levels(caches: &str) -> Vec<String> {
    const NEEDLE: &str = "\"level\":";
    let mut levels = Vec::new();
    let mut rest = caches;

    while let Some(at) = rest.find(NEEDLE) {
        let after = &rest[at + NEEDLE.len()..];
        let end = after.find([',', '}']).unwrap_or(after.len());
        levels.push(after[..end].trim().to_owned());
        rest = &after[end..];
    }

    levels
}
/// Push a disagreement when both renderings are present and differ.
/// One prose reading against its machine-readable counterpart.
///
/// **Absence is reported HERE, so every rule inherits it.** Each caller reaches
/// this only after finding the prose, so a `None` counterpart is not "the report
/// does not mention this fact" -- it is "the report states this fact once and
/// the row lost it". Returning early on `None` made that silent for every
/// comparison routed through this helper at once.
///
/// Fixing it at the three reported call sites first, rather than here, is what
/// left the rest: a later review named five more, and a deletion sweep of the
/// real report then found twelve keys whose removal the oracle accepted. The
/// helper is the only place the rule cannot be forgotten for the next fact
/// somebody adds.
fn compare(found: &mut Vec<Correspondence>, fact: &'static str, prose: &str, json: Option<&str>) {
    let Some(json) = json else {
        found.push(Correspondence::RenderedOnlyInProse {
            fact,
            prose: prose.to_owned(),
        });
        return;
    };
    if prose != json {
        found.push(Correspondence::ProseAndNdjsonDisagree {
            fact,
            prose: prose.to_owned(),
            ndjson: json.to_owned(),
        });
    }
}

/// `(policy name, domain count)` for each row of the policy table.
fn policy_rows(report: &str) -> Vec<(String, String)> {
    report
        .lines()
        .skip_while(|line| !line.starts_with("domains each policy would produce:"))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.to_owned(), parts.next()?.to_owned()))
        })
        .collect()
}

/// The object in the `caches` array that names `level`, if one does.
///
/// Compares whole members rather than searching for a prefix, so `"level":1`
/// does not match the object for level 10, and does not care what order the
/// members are written in.
fn cache_object<'a>(caches: &'a str, level: &str) -> Option<&'a str> {
    let named = format!("\"level\":{level}");

    caches.split('{').find_map(|chunk| {
        let object = chunk.split('}').next()?;

        object
            .split(',')
            .any(|member| member.trim() == named)
            .then_some(object)
    })
}

/// `(level, domain count)` for each row of the cache table.
fn cache_rows(report: &str) -> Vec<(String, String)> {
    report
        .lines()
        .skip_while(|line| !line.starts_with("caches:"))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let level = parts.next()?.strip_prefix('L')?.to_owned();
            Some((level, parts.next()?.to_owned()))
        })
        .collect()
}

/// The independently-read Win32 counters against the verdict drawn from them.
///
/// A different shape from the rules above, and the one closest to what this
/// probe is *for*. The prose prints each counter beside the enumerated value it
/// was read to check; the whole point of the run is that a mismatch is a
/// finding. So a counter that disagrees with the enumeration while the verdict
/// reads `agree` is the original defect in its purest form -- the report
/// showing its own contradicting evidence directly above a verdict denying it.
fn check_counters_against_verdict(
    report: &str,
    ndjson: Option<&str>,
    found: &mut Vec<Correspondence>,
) {
    let Some(ndjson) = ndjson else {
        return;
    };
    if ndjson_field(ndjson, "cross_check") != Some("agree") {
        return;
    }

    // Only the two counters that are a direct count of an enumerated quantity.
    // `GetNumaHighestNodeNumber` is deliberately absent: it reports the largest
    // node NUMBER, which the report itself says is not a count, so comparing it
    // against `numa_domains` would manufacture a disagreement on any machine
    // with sparse node numbering.
    for (label, key, fact) in [
        (
            "  GetActiveProcessorCount     : ",
            "processors",
            "active processor count against the enumeration",
        ),
        (
            "  GetActiveProcessorGroupCount: ",
            "groups",
            "active group count against the enumeration",
        ),
    ] {
        // **Reached only under an agreeing verdict**, which is what makes the
        // absence of either side a contradiction rather than silence: `agree`
        // asserts the check was MADE, and the counter line is the evidence it
        // was. Skipping quietly accepted a report that claimed a check it did
        // not show -- measured, by deleting the `GetActiveProcessorCount` line
        // and watching `check` return nothing.
        let (Some(counter), Some(enumerated)) =
            (prose_field(report, label), ndjson_field(ndjson, key))
        else {
            found.push(Correspondence::EvidenceMissingWithAgreeingVerdict { fact });
            continue;
        };
        if counter != enumerated {
            found.push(Correspondence::ProseAndNdjsonDisagree {
                fact,
                prose: counter.to_owned(),
                ndjson: enumerated.to_owned(),
            });
        }
    }
}

/// A list of numbers as a comparable string, whichever way it was punctuated.
///
/// **Exactly one outer pair of brackets comes off, so nesting survives the
/// normalisation.** `trim_matches` removes every consecutive bracket, which
/// collapsed `[0]` and `[[0]]` to the same `0` -- so a renderer that regressed
/// to a nested array beside one-level prose would have compared equal. The
/// punctuation this is meant to forgive is one side writing `[0, 1]` where the
/// other writes `0,1`; a difference in DEPTH is a real disagreement and must
/// survive to be reported.
fn normalise_list(rendered: &str) -> String {
    rendered
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(rendered)
        .split(',')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// Hardware claims the prose can make, each with the caveat that must accompany
/// it when the parse is in doubt.
const GATED_CLAIMS: &[(&str, &str, &str)] = &[(
    "(heterogeneous: an I/O thread left unconstrained can land on an",
    "This run did not establish that the parse is whole",
    "heterogeneity",
)];

fn check_claims_against_doubt(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let Some(ndjson) = ndjson else {
        return;
    };

    // The report's own visible evidence that its parse was in doubt.
    //
    // These two are exactly what `CrossCheck::parse_in_doubt` is defined as --
    // a non-empty `parse_incomplete` or a non-empty `disagreements`, the latter
    // being what makes the verdict `disagree`. That correspondence is the point
    // rather than a coincidence: if the definition changes and this does not,
    // the sabotage check in M2.2 is what should notice.
    let mut evidence = Vec::new();
    if let Some(count) = ndjson_field(ndjson, "parse_incomplete")
        && count != "0"
    {
        evidence.push(format!("parse_incomplete={count}"));
    }
    if ndjson_field(ndjson, "cross_check") == Some("disagree") {
        evidence.push("cross_check=disagree".to_owned());
    }

    if evidence.is_empty() {
        return;
    }

    for (claim, caveat, name) in GATED_CLAIMS {
        // The CLAIM is anchored and the CAVEAT is not, deliberately. A claim
        // found where none was made invents a violation, which is the direction
        // that must not be fooled by embedded text; a caveat found where none
        // was made only SUPPRESSES one. And the caveat token is written to match
        // mid-line on purpose -- the renderer prints it after `(of the levels
        // that decoded. ` in one arm and after `(` in another -- so anchoring it
        // would stop it matching the lines it exists for.
        // **Scoped to the claim's OWN block, not the whole report.** A review
        // predicted that another block's caveat could satisfy this one: the
        // `Level` cache arm under `parse_in_doubt` writes the same sentence, and
        // a report can be heterogeneous and in doubt and take that arm at once.
        //
        // Measured on exactly that crossed shape: it does NOT mask, because the
        // renderer WRAPS the cache arm's sentence -- `... did not establish that
        // the parse` ends one line and `is whole ...` begins the next -- so no
        // single line carries the token. The finding was wrong about today's
        // renderer and right about the code: that protection is an accident of
        // where a line happens to break, and reflowing that sentence would
        // silently turn the oracle blind to an uncaveated hardware claim.
        //
        // The block is the claim's line and the indented continuation under it,
        // which is what the renderer actually emits and what the caveat belongs
        // to. An accident that holds is still an accident.
        if has_line_beginning(report, claim)
            && !claim_block(report, claim).any(|line| line.contains(caveat))
        {
            found.push(Correspondence::UncaveatedClaimUnderDoubt {
                claim: name,
                evidence: evidence.join(", "),
            });
        }
    }
}

#[cfg(test)]
mod tests;
