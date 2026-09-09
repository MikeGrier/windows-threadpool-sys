// Copyright (c) Mike Grier.
// Split from bin/topology.rs at c335ae5.

//! The topology probe's report, as text.
//!
//! Split out of the binary because it was untestable there: `render` called
//! [`crate::topology::measure`] itself, so every branch needed a live host and
//! none could be
//! driven from a test. A mutation sweep found 13 of 13 mutants surviving --
//! `render` could return `"xyzzy"` and the suite stayed green -- and the
//! survivors were exactly the claims that cost the most review rounds: the
//! "no L3 at all" note, the caveat gate on an absent partitioning answer, and
//! the heterogeneous-core note. Each had been checked by running the binary and
//! reading the output, which nothing repeats on a later change.
//!
//! The same split as [`crate::topology::observe`] out of
//! [`crate::topology::measure`], for the same reason.

use std::fmt::Write as _;
use std::io;

use windows_placement_probe::fingerprint::{Fingerprint, banner_line_for};

use crate::topology::{Observation, PartitioningCache, Verdict};

/// The banner and title both reports open with.
///
/// The banner is part of the returned text rather than written out separately:
/// a captured report must carry the line naming the machine that produced it,
/// and the taint marker with it. Without it a number can be pasted anywhere and
/// compared against anything.
///
/// **Taken as an argument rather than read here.** This called
/// `fingerprint::banner_line()`, which runs a topology discovery of its own --
/// so this module's claim to be testable without a host was false of its very
/// first line, and the probe made a second, unbracketed platform read whose
/// result could describe a different instant from the body's. The caller reads
/// it once and passes it in.
fn preamble(banner: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{banner}");
    let _ = writeln!(
        out,
        "== processor topology, and what each partitioning policy would yield ==\n"
    );
    out
}

/// The banner to print, given the host fingerprint read before and after the
/// measurement.
///
/// The fingerprint is itself a topology rendering -- architecture, processor and
/// core counts, cache domain sizes, NUMA nodes -- read through a *second*
/// discovery that `measure`'s bracket does not enclose. Reading it once and
/// calling that "attribution" was the weaker claim it sounded: a machine that
/// changed across the run would print one shape in the banner and a different
/// one in the body, with nothing saying which described the measurement.
///
/// So it is bracketed like everything else here. Equal readings print as one
/// line, exactly as before. Readings that differ print both and say so, because
/// which of them describes the body is what the run did not establish.
///
/// **It says WHICH reading is unknown, not that both are wrong.** The
/// measurement happened between them, so one of the two may well name the
/// machine the body describes -- what a disagreement at the endpoints
/// establishes is that this run cannot say which. Calling both wrong would be
/// its own over-claim, in the sentence added to stop one.
///
/// **It says the readings DIFFER, not that the host changed.**
/// `Fingerprint::discover` returns `Ok` on a parse that dropped a record or
/// whose two sources disagreed, so a fingerprint can differ from the one before
/// it because the enumeration was flaky rather than because any hardware moved.
/// Naming a cause this run cannot distinguish would be the same over-claim the
/// body below is built to avoid; what is established is that the two disagree,
/// and that is all this says.
///
/// **Takes the discoveries rather than two rendered lines.** Compared as
/// strings, a failed read is a line like any other, so one failure beside one
/// success -- or two failures whose `io::Error` text differs -- read as a host
/// that changed. That is a claim about the machine drawn from a gap in the
/// measurement: a failed read establishes neither that the host moved nor that
/// it held still.
#[must_use]
pub fn attribution(before: &io::Result<Fingerprint>, after: &io::Result<Fingerprint>) -> String {
    let first = banner_line_for(before);
    match (before, after) {
        (Ok(one), Ok(two)) if one == two => first,
        (Ok(_), Ok(_)) => format!(
            "{first}\n{}\nHOST READINGS DISAGREE: the two readings above bracket the measurement\n\
             and differ, so which of them names the machine the body below describes\n\
             was not established.",
            banner_line_for(after)
        ),
        // Covers (Err, Ok), (Ok, Err) AND (Err, Err), so the text says "at
        // least one". "One of the two readings failed" understates the case
        // where both did -- a small thing, but the same shape as every other
        // sentence corrected here: claiming a more specific state than the run
        // established.
        _ => format!(
            "{first}\n{}\nHOST NOT ESTABLISHED: at least one of the two readings that bracket \
             the measurement\nfailed, so nothing confirmed the machine held still under it.",
            banner_line_for(after)
        ),
    }
}

/// The report for a run whose discovery failed.
///
/// Carries an `x-probe-topology` row of its own, so the fleet survey can tell a
/// host where discovery FAILED from a job that never ran the probe. Returning
/// only prose made those indistinguishable, which silently excluded exactly the
/// hosts most worth counting.
#[must_use]
pub fn report_unmeasured(banner: &str, error: &io::Error) -> String {
    let mut out = preamble(banner);
    let _ = writeln!(out, "MachineMemoryTopology::discover failed: {error}");
    let _ = writeln!(
        out,
        "(Reported rather than measured: a probe that cannot read its"
    );
    let _ = writeln!(
        out,
        "subject must say so instead of printing a misleading shape.)"
    );
    let _ = writeln!(
        out,
        r#"{{"reason":"x-probe-topology","arch":"{}","cross_check":"not_measured"}}"#,
        std::env::consts::ARCH
    );
    out
}

/// The report for a run whose discovery succeeded.
#[must_use]
pub fn report(banner: &str, observation: &Observation) -> String {
    let mut out = preamble(banner);

    // Computed HERE rather than beside the cache conclusions it was first
    // written for. The heterogeneity note below is a hardware claim too -- "an
    // unconstrained thread can land on an efficiency core" -- and it was printed
    // before this line existed, so it was structurally ungated while the design
    // note said `parse_in_doubt` gates every hardware conclusion the renderer
    // draws. A rule with one exception is not a rule, and the exception was the
    // conclusion drawn from the one field the two sources are known to
    // contradict each other about.
    let check = observation.cross_check();
    let parse_in_doubt = check.parse_in_doubt();

    let _ = writeln!(
        out,
        "processors (online) : {}",
        observation.online_processors
    );
    let _ = writeln!(out, "processor groups    : {}", observation.groups);
    let _ = writeln!(out, "packages            : {}", observation.packages);
    let _ = writeln!(
        out,
        "NUMA domains        : {} ({} with no processors)",
        observation.numa_domains, observation.numa_domains_without_processors
    );
    if observation.numa_domains_only_in_cpu_sets > 0 {
        let _ = writeln!(
            out,
            "  ({} reported only by CPU Sets, never by the relationship walk:",
            observation.numa_domains_only_in_cpu_sets
        );
        let _ = writeln!(out, "   the two sources group nodes differently)");
    }
    let _ = writeln!(out, "physical cores      : {}", observation.cores.len());

    let smt = observation
        .cores
        .iter()
        .filter(|c| c.simultaneous_multithreading)
        .count();
    let mut classes: Vec<u8> = observation
        .cores
        .iter()
        .map(|c| c.efficiency_class)
        .collect();
    classes.sort_unstable();
    classes.dedup();
    let _ = writeln!(out, "  cores with SMT    : {smt}");
    let _ = writeln!(out, "  efficiency classes: {classes:?}");
    if classes.len() > 1 {
        let _ = writeln!(
            out,
            "  (heterogeneous: an I/O thread left unconstrained can land on an"
        );
        let _ = writeln!(
            out,
            "   efficiency core, which is why even a single domain wants a mask)"
        );
        if parse_in_doubt {
            let _ = writeln!(
                out,
                "   (This run did not establish that the parse is whole, and the classes"
            );
            let _ = writeln!(
                out,
                "    above are what decoded -- see the cross-check below for why.)"
            );
        }
    }

    let _ = writeln!(out, "\ncaches:");
    if observation.caches.is_empty() {
        let _ = writeln!(out, "  none reported");
    }
    for cache in &observation.caches {
        let _ = writeln!(
            out,
            "  L{:<2} {:>3} domain(s), processors per domain: {:?}",
            cache.level,
            cache.domains(),
            cache.processors_per_domain
        );
    }

    // Every conclusion below is drawn from the cache summaries above, and those
    // describe what DECODED rather than what the machine has. An undersized
    // `CACHE_RELATIONSHIP` decodes to nothing and is recorded as an anomaly,
    // while one whose trailing affinity array is truncated is KEPT with the
    // entries that fit -- and `discover` returns `Ok` either way. So the
    // summaries can be short a level or carry one whose processor set is
    // smaller than the truth, and these lines say what the parse contains
    // rather than what the hardware is.
    //
    // The truncated case is worth stating precisely, because an earlier version
    // of this comment said it "decodes to nothing": a kept record with a
    // partial mask presents a set that is distinct from the full one, so it can
    // ADD a partition rather than remove one. A maintainer who believed the old
    // wording would have ruled out the only way that inflation happens.
    //
    // Named for the CONDITION, not for one of its causes. It was `dropped`, and
    // the caveats it gated said "a record was dropped" -- which became false the
    // moment the condition widened to every `parse_incomplete` entry: forcing a
    // coherence disagreement with zero anomalies printed a caveat blaming a
    // dropped record. Each caveat now states that the run did not establish the
    // parse is whole and points at the cross-check, which is where the actual
    // reason is already printed in full.
    //
    // ASKED, not re-derived, because every time this condition has been
    // restated here it has been restated wrongly. First as
    // `!enumeration_anomalies.is_empty()`, so a host reporting
    // `Coherence::Disagreed` with no anomalies printed "this machine reports no
    // L3 at all" a few lines above "=> INCOMPLETE ... its two enumerations never
    // agreed". Then as `!parse_incomplete.is_empty()`, which missed
    // `disagreements` -- so a host whose group count Windows contradicts printed
    // the same hardware claim directly above "=> DISAGREE", in the one case
    // where the evidence that the parse does not describe this machine was
    // already in hand. Both times the accompanying comment asserted the
    // condition was complete.
    //
    // `CrossCheck::parse_in_doubt` is now the single definition, and it is the
    // place that argues which lists belong. It is computed at the top of this
    // function so that every hardware conclusion below reads the same one.

    // Matched exhaustively, so an absent level cannot be printed without having
    // decided WHICH absent case it is. This arm used to recite the ambiguity --
    // "either no level partitions this machine, or two partition it
    // incomparably" -- because the probe genuinely could not tell. It can tell
    // the first apart from the rest, so it now says so.
    match observation.partitioning_cache() {
        PartitioningCache::Level(cache) => {
            // "of the processors it covers", not "of this machine". The owning
            // crate's filter is `blocks.len() > 1 && are_pairwise_disjoint`,
            // with no coverage check, so two disjoint domains covering
            // processors 0 and 1 of a four-processor host qualify -- and
            // nothing here verifies otherwise. The mirror sentence in the
            // `NoLevelPartitions` arm below was narrowed for this exact reason;
            // this one kept the claim, which is the half of a pair being swept
            // and the other half left behind.
            let _ = writeln!(
                out,
                "\noutermost cache that partitions the processors it covers: L{} ({} domains)",
                cache.level,
                cache.domains()
            );
            if parse_in_doubt {
                let _ = writeln!(
                    out,
                    "  (of the levels that decoded. This run did not establish that the parse"
                );
                let _ = writeln!(
                    out,
                    "   is whole -- see the cross-check below for why -- so a coarser level may"
                );
                let _ = writeln!(out, "   exist on this machine and be missing above.)");
            }
        }
        PartitioningCache::NoLevelPartitions => {
            let _ = writeln!(
                out,
                "\nno cache level reported more than one domain, so nothing here divides"
            );
            // Not "every level covers the whole machine", which was the reading
            // this offered and is false for a level that decoded to NO
            // partitions -- that covers nothing, not everything, and lands here
            // too. `cross_check` now caveats that case, but the sentence should
            // not have needed the caveat to stop being wrong.
            //
            // Nor "no cache boundary divides the work", which was the next
            // wording and is a claim about the HARDWARE. A single domain
            // covering half the online processors also lands here, and nothing
            // in this probe checks that the domains at a level cover the
            // machine -- so the honest statement is about what this report
            // found, not about what the silicon does.
            let _ = writeln!(out, "the work by cache.");
        }
        PartitioningCache::NoLevelsReported => {
            let _ = writeln!(
                out,
                "\nno cache levels were reported at all, so nothing here says whether a"
            );
            let _ = writeln!(out, "cache boundary divides this machine.");
        }
        PartitioningCache::NoUniqueOutermost => {
            // "More than one DISTINCT domain", not "partitions this machine".
            // `domains()` counts distinct processor sets and the topology crate
            // is explicit that distinct is not disjoint, so a level whose blocks
            // overlap lands here while partitioning nothing -- which the third
            // line below then names, contradicting an opening that claimed the
            // machine was partitioned.
            let _ = writeln!(
                out,
                "\nat least one cache level reported more than one distinct domain, but"
            );
            let _ = writeln!(
                out,
                "no unique outermost one was established: either two partition this"
            );
            let _ = writeln!(
                out,
                "machine incomparably, or the candidates were rejected as overlapping"
            );
            let _ = writeln!(out, "-- in which case none of them partitions it at all.");
        }
        PartitioningCache::SummaryMissing(level) => {
            let _ = writeln!(
                out,
                "\nBUG IN THIS PROBE: the topology crate named L{level} as the outermost"
            );
            let _ = writeln!(
                out,
                "partitioning cache and this survey carries no summary for it. Nothing"
            );
            let _ = writeln!(out, "below about cache partitioning can be trusted.");
        }
    }
    if parse_in_doubt
        && !matches!(
            observation.partitioning_cache(),
            PartitioningCache::Level(_)
        )
    {
        // A parse that is short or disputed is exactly how the level that would
        // have partitioned this machine goes missing, so none of the three
        // absent answers above is safe to read as hardware either.
        let _ = writeln!(
            out,
            "Or the parse is not whole and the level that would have partitioned this"
        );
        let _ = writeln!(
            out,
            "machine is missing -- see the cross-check below for why."
        );
    }
    if !observation.caches.iter().any(|c| c.level == 3) {
        // "no L3" is a claim about the machine, and the list it is read off is
        // only what decoded. Asserted as hardware when the parse is whole, and
        // as a fact about the parse when it is not -- otherwise a host whose L3
        // record alone failed to decode is filed as an ARM64-style no-L3
        // machine, and its "outermost partitioning cache: L2" is read as a real
        // cluster boundary.
        if parse_in_doubt {
            let _ = writeln!(
                out,
                "NOTE: no L3 decoded on this run, and this run did not establish that the"
            );
            let _ = writeln!(
                out,
                "parse is whole -- so that is a fact about the parse, not evidence the"
            );
            let _ = writeln!(out, "machine has no L3. See the cross-check below.");
        } else {
            let _ = writeln!(
                out,
                "NOTE: this machine reports no L3 at all, so a policy keyed literally"
            );
            let _ = writeln!(
                out,
                "on \"L3\" would find nothing here. That is the measured case behind"
            );
            let _ = writeln!(
                out,
                "phrasing the rule as \"the outermost level that partitions\"."
            );
        }
    }

    let _ = writeln!(out, "\ndomains each policy would produce:");
    for (name, count) in observation.domain_counts() {
        let _ = writeln!(out, "  {name:<34} {count}");
    }

    let _ = writeln!(
        out,
        "\ncross-check against independently read Win32 counters:"
    );
    let _ = writeln!(
        out,
        "  GetActiveProcessorCount     : {}",
        observation.raw_active_processors
    );
    let _ = writeln!(
        out,
        "  GetActiveProcessorGroupCount: {}",
        observation.raw_group_count
    );
    match observation.raw_highest_numa_node {
        Some(highest) => {
            // The identifier, and deliberately no count derived from it.
            // `GetNumaHighestNodeNumber` reports the largest node NUMBER, and
            // node numbers may be sparse -- a machine with nodes 0 and 2 has two
            // nodes and a highest of 2. `highest + 1` would print three, which
            // is the same mistake `Observation::cross_check` was corrected to
            // stop making; re-deriving it here would put it back in the output
            // the cross-check is printed beside.
            let _ = writeln!(out, "  GetNumaHighestNodeNumber    : {highest}");
            let _ = writeln!(
                out,
                "    (the largest node NUMBER, not a count: node numbers can be sparse)"
            );
        }
        None => {
            let _ = writeln!(out, "  GetNumaHighestNodeNumber    : failed");
        }
    }
    // Matched exhaustively on purpose. The verdict used to be `complaints
    // .is_empty()`, which printed "agree" when a counter had merely failed to
    // read -- on the line directly below "GetNumaHighestNodeNumber : failed".
    // A three-state verdict makes that arm impossible to omit.
    match check.verdict() {
        Verdict::Agree => {
            let _ = writeln!(
                out,
                "  => agree. Every check this probe could make was made and matched."
            );
        }
        Verdict::Disagree => {
            let _ = writeln!(out, "  => DISAGREE. This is a finding, not a nuisance:");
            for complaint in &check.disagreements {
                let _ = writeln!(out, "     - {complaint}");
            }
            // Listed even here, so a reader knows the disagreement above is not
            // the whole picture.
            for skipped in &check.not_compared {
                let _ = writeln!(out, "     (not compared) {skipped}");
            }
            for caveat in &check.parse_incomplete {
                let _ = writeln!(out, "     (parse incomplete) {caveat}");
            }
        }
        Verdict::Incomplete => {
            let _ = writeln!(
                out,
                "  => INCOMPLETE. Nothing this probe compared disagreed, but this run"
            );
            let _ = writeln!(out, "     did not establish that the parse is consistent:");
            for skipped in &check.not_compared {
                let _ = writeln!(out, "     - {skipped}");
            }
            for caveat in &check.parse_incomplete {
                let _ = writeln!(out, "     - {caveat}");
            }
        }
    }

    // One machine-readable line, so accumulated CI logs can be mined without
    // parsing the prose above. Kept to a single line on purpose.
    //
    // `cross_check` is the one field a mining pass must read before trusting
    // any other. Every count here is taken from what decoded, so a dropped
    // record makes `caches`, `packages`, `cores` and
    // `outermost_partitioning_cache_level` short by an amount no field states
    // -- and a query grouping by cache level has no reason to join against
    // `enumeration_anomalies` on its own. `cross_check` closes that: anomalies
    // populate `parse_incomplete`, and a non-empty `parse_incomplete` forces
    // the verdict away from "agree", so `cross_check == "agree"` IMPLIES no
    // record failed to decode. One way only: a run whose counter failed to
    // read has a complete parse and still reports "incomplete". That is a rule about
    // the code above, so the test
    // `a_dropped_enumeration_record_blocks_agreement_even_when_every_counter_matches`
    // pins it rather than leaving it as a promise in a comment.
    //
    // That is narrower than "the counts are complete", and the gap is not
    // closed anywhere: nothing independent measures packages, cores or caches,
    // so a record that decoded cleanly while describing less of the machine
    // than exists raises no anomaly and reaches "agree". `agree` says every
    // check this probe could make was made and matched -- not that a check
    // exists for every field on this line.
    //
    // "Not short" is all it says, and the distinction is load-bearing. This
    // comment used to promise that `agree` made the rest of the line "describe
    // the machine rather than the parse", which was false for
    // `outermost_partitioning_cache_level`: a machine whose levels partition it
    // incomparably has a complete parse, agrees with every counter, and still
    // cannot be said to have an outermost partitioning cache. It emitted `null`
    // exactly like a machine no level partitions -- opposite conclusions for
    // anything sizing itself by cache boundary, on a row already certified.
    // Hence the `outermost_partitioning_cache` field beside it, which a
    // consumer must read rather than inferring from the level being absent.
    let cache_json: Vec<String> = observation
        .caches
        .iter()
        .map(|c| format!(r#"{{"level":{},"domains":{}}}"#, c.level, c.domains()))
        .collect();
    let policy_json: Vec<String> = observation
        .domain_counts()
        .into_iter()
        .map(|(name, count)| format!(r#""{name}":{count}"#))
        .collect();
    let _ = writeln!(
        out,
        concat!(
            r#"{{"reason":"x-probe-topology","arch":"{}","processors":{},"groups":{},"#,
            r#""packages":{},"numa_domains":{},"numa_domains_without_processors":{},"cores":{},"#,
            r#""efficiency_classes":{},"caches":[{}],"outermost_partitioning_cache_level":{},"#,
            r#""outermost_partitioning_cache":"{}","#,
            r#""policies":{{{}}},"cross_check":"{}","not_compared":{},"parse_incomplete":{},"#,
            r#""enumeration_anomalies":{},"numa_domains_only_in_cpu_sets":{}}}"#
        ),
        std::env::consts::ARCH,
        observation.online_processors,
        observation.groups,
        observation.packages,
        observation.numa_domains,
        observation.numa_domains_without_processors,
        observation.cores.len(),
        classes.len(),
        cache_json.join(","),
        observation
            .outermost_partitioning_cache()
            .map_or("null".to_string(), |c| c.level.to_string()),
        // The level alone said `null` for every absent case alike, on a line
        // the verdict had already certified as "agree" -- an incomparable
        // partitioning touches nothing `cross_check` consults. A query counting
        // nulls as "machines no cache level partitions" then folded in machines
        // where a level DOES partition, which is the opposite conclusion for
        // anything sizing itself by cache boundary. Always a string, so a
        // consumer filters on `== "none"` rather than on the absence of a
        // number.
        match observation.partitioning_cache() {
            PartitioningCache::Level(_) => "level",
            PartitioningCache::NoLevelsReported => "no_levels_reported",
            PartitioningCache::NoLevelPartitions => "none",
            PartitioningCache::NoUniqueOutermost => "not_unique",
            PartitioningCache::SummaryMissing(_) => "summary_missing",
        },
        policy_json.join(","),
        // A tri-state rather than a boolean, for the reason the prose above
        // gives: a log-mining pass over accumulated CI output must be able to
        // tell "all three counters agreed" from "two agreed and the third was
        // never compared". `cross_check_ok:true` said the same thing for both.
        match check.verdict() {
            Verdict::Agree => "agree",
            Verdict::Disagree => "disagree",
            Verdict::Incomplete => "incomplete",
        },
        check.not_compared.len(),
        // Separate from `not_compared`, because a mining pass that finds
        // `"cross_check":"incomplete"` needs to know whether this probe failed
        // to read a counter or the parse itself was short or disputed -- the
        // first is a gap in the measurement, the second a fact about the
        // machine worth going and looking at. The two counts beside it say
        // which kind, without a consumer having to know what `cross_check`
        // currently pushes for.
        check.parse_incomplete.len(),
        observation.enumeration_anomalies.len(),
        observation.numa_domains_only_in_cpu_sets,
    );
    out
}
