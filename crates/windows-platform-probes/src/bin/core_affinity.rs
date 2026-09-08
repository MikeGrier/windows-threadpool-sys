// Copyright (c) Mike Grier.

//! Prints whether it matters where the two ends of a queue run.

use std::fmt::Write as _;

use windows_placement_probe::core_affinity::{Observation, Placement, measure};
use windows_placement_probe::peer_index_cache::Strategy;
use windows_platform_probes::report::{Stdout, emit};
use windows_topology_sys::Observed;

fn main() -> std::io::Result<()> {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(&mut Stdout, &render(&measure()?));
    Ok(())
}

/// The probe's whole report, as text.
fn render(observation: &Observation) -> String {
    let mut out = String::new();
    // First line of the report, and part of the returned text rather than
    // written out here: a captured report must carry the line naming the
    // machine that produced it, and the taint marker with it.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    let _ = writeln!(
        out,
        "== does it matter where the two ends of a queue run? ==\n"
    );

    let _ = writeln!(out, "processors, as discovered:");
    let _ = writeln!(
        out,
        "  {:>8}  {:>16}  {:>13}",
        "cpu", "efficiency class", "cache domain"
    );
    for place in &observation.processors {
        // Group and number together: a number is unique only within its group,
        // so two distinct processors on a machine with more than 64 of them
        // would otherwise both render as `cpu5`.
        let _ = writeln!(
            out,
            "  {:>8}  {:>16}  {:>13}",
            format!("g{}/cpu{}", place.group, place.number),
            place.efficiency_class,
            match place.cache_domain {
                Observed::Known(id) => id.to_string(),
                Observed::Absent => "none".to_owned(),
                Observed::NotObserved => "unknown".to_owned(),
            }
        );
    }

    let classes: Vec<u8> = {
        let mut seen: Vec<u8> = observation
            .processors
            .iter()
            .map(|p| p.efficiency_class)
            .collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    };
    let _ = writeln!(
        out,
        "\n  {} efficiency class(es), {} cache domain(s)",
        classes.len(),
        {
            let mut seen: Vec<_> = observation
                .processors
                .iter()
                .map(|p| p.cache_domain)
                .collect();
            seen.sort_unstable();
            seen.dedup();
            seen.len()
        }
    );

    if !observation.by_class.is_empty() {
        let _ = writeln!(
            out,
            "\n-- the same handoff, within each efficiency class --"
        );
        let _ = writeln!(
            out,
            "{:<12} {:>8} {:>8} {:>12} {:>12} {:>10}",
            "class", "prod", "cons", "base ns/it", "cached ns/it", "cach depth"
        );
        let mut classes: Vec<u8> = observation
            .by_class
            .iter()
            .map(|m| m.producer.efficiency_class)
            .collect();
        classes.sort_unstable();
        classes.dedup();
        for class in classes {
            let base = observation
                .by_class
                .iter()
                .find(|m| m.producer.efficiency_class == class && m.strategy == Strategy::Baseline);
            let cached = observation
                .by_class
                .iter()
                .find(|m| m.producer.efficiency_class == class && m.strategy == Strategy::Cached);
            if let (Some(base), Some(cached)) = (base, cached) {
                let _ = writeln!(
                    out,
                    "{:<12} {:>8} {:>8} {:>12.1} {:>12.1} {:>10.1}",
                    format!("class {class}"),
                    format!("g{}/cpu{}", base.producer.group, base.producer.number),
                    format!("g{}/cpu{}", base.consumer.group, base.consumer.number),
                    base.nanos_per_item,
                    cached.nanos_per_item,
                    cached.consumer_batch
                );
            }
        }
        let _ = writeln!(
            out,
            "  (Windows numbers efficiency classes with the FASTER cores higher, so\n   \
             the highest class here is the performance one.)"
        );
    }

    let _ = writeln!(out, "\n-- the handoff, by placement --");
    let _ = writeln!(
        out,
        "{:<26} {:>8} {:>8} {:>12} {:>12} {:>10} {:>10}",
        "placement", "prod", "cons", "base ns/it", "cached ns/it", "base depth", "cach depth"
    );

    // Every variant, tightest coupling first. `SameCoreSiblings` MUST be here:
    // it is the placement the caching hypothesis is about, and on an SMT host it
    // is where the interesting result lives. Omitting it once already produced a
    // table that disagreed with the interpretation printed directly beneath it.
    let all = [
        Placement::SameCoreSiblings,
        Placement::SameCacheSameClass,
        Placement::SameCacheCrossClass,
        Placement::CrossCacheSameClass,
        Placement::CrossCacheCrossClass,
        Placement::CrossNumaNode,
    ];

    for placement in all {
        let (Some(base), Some(cached)) = (
            observation.get(placement, Strategy::Baseline),
            observation.get(placement, Strategy::Cached),
        ) else {
            // Absent is a finding, not a gap: it means this machine cannot
            // express the placement at all.
            let _ = writeln!(
                out,
                "{:<26} {:>8} {:>8} {:>12} {:>12} {:>10} {:>10}",
                placement.label(),
                "-",
                "-",
                "n/a",
                "n/a",
                "-",
                "-"
            );
            continue;
        };
        let _ = writeln!(
            out,
            "{:<26} {:>8} {:>8} {:>12.1} {:>12.1} {:>10.1} {:>10.1}",
            placement.label(),
            format!("g{}/cpu{}", base.producer.group, base.producer.number),
            format!("g{}/cpu{}", base.consumer.group, base.consumer.number),
            base.nanos_per_item,
            cached.nanos_per_item,
            base.consumer_batch,
            cached.consumer_batch
        );
    }

    let _ = writeln!(out, "\nthe slice each row was measured on:");
    for placement in all {
        if let Some(base) = observation.get(placement, Strategy::Baseline) {
            let _ = writeln!(out, "  {:<26} {}", placement.label(), base.slice);
        }
    }

    render_node_distances(&mut out, observation);

    let _ = writeln!(
        out,
        "
interpretation:
"
    );

    let expressible = observation.placements();
    if expressible.len() < 2 {
        let _ = writeln!(
            out,
            "  This machine expresses only one placement, so it cannot answer"
        );
        let _ = writeln!(
            out,
            "  the question. That is a fact about the host, not a null result:"
        );
        let _ = writeln!(
            out,
            "  a homogeneous single-cache machine has nowhere else to put the"
        );
        let _ = writeln!(out, "  two threads.");
        return out;
    }

    // Whether the two factors can be told apart at all on this host. If every
    // cross-class pair is also cross-cache, they are perfectly confounded and
    // no amount of measurement here separates them -- which is a fact to state,
    // not to reason past.
    let confounded = !expressible.contains(&Placement::SameCacheCrossClass)
        && !expressible.contains(&Placement::CrossCacheSameClass);
    if confounded {
        let _ = writeln!(
            out,
            "  CAUTION: on this machine the efficiency classes and the cache"
        );
        let _ = writeln!(
            out,
            "  domains coincide exactly, so every cross-class pair is also a"
        );
        let _ = writeln!(
            out,
            "  cross-cache pair. The two effects are perfectly CONFOUNDED here"
        );
        let _ = writeln!(
            out,
            "  and nothing below separates them. Read the rows as 'within a"
        );
        let _ = writeln!(
            out,
            "  domain' versus 'across domains', and do not attribute the"
        );
        let _ = writeln!(
            out,
            "  difference to core speed or to cache without a machine whose"
        );
        let _ = writeln!(out, "  classes and caches cut differently.\n");
    }

    // Batch depth is read from the CACHED runs, never the baseline ones.
    // Baseline reads the shared line on every operation by definition, so its
    // depth is ~1 whatever the placement and carries no information at all. An
    // earlier version of this probe compared the baseline depths and duly
    // reported ~0.8 against ~0.4, which is noise around a constant being read
    // as a finding.
    let same_class: Vec<_> = expressible
        .iter()
        .filter(|p| {
            matches!(
                p,
                Placement::SameCacheSameClass | Placement::CrossCacheSameClass
            )
        })
        .filter_map(|p| observation.get(*p, Strategy::Cached))
        .collect();
    let cross_class: Vec<_> = expressible
        .iter()
        .filter(|p| {
            matches!(
                p,
                Placement::SameCacheCrossClass | Placement::CrossCacheCrossClass
            )
        })
        .filter_map(|p| observation.get(*p, Strategy::Cached))
        .collect();

    let mean = |runs: &[_], f: fn(&_) -> f64| -> Option<f64> {
        if runs.is_empty() {
            None
        } else {
            Some(runs.iter().map(f).sum::<f64>() / runs.len() as f64)
        }
    };

    if let (Some(same), Some(cross)) = (
        mean(
            &same_class,
            |m: &windows_placement_probe::core_affinity::Measurement| m.consumer_batch,
        ),
        mean(
            &cross_class,
            |m: &windows_placement_probe::core_affinity::Measurement| m.consumer_batch,
        ),
    ) {
        let within = if confounded {
            "within a domain "
        } else {
            "same-class  "
        };
        let across = if confounded {
            "across domains "
        } else {
            "cross-class "
        };
        let _ = writeln!(
            out,
            "  batch depth with caching on, {within}: {same:.1} items per shared read"
        );
        let _ = writeln!(
            out,
            "  batch depth with caching on, {across}: {cross:.1} items per shared read"
        );
        if cross > same * 2.0 {
            let _ = writeln!(
                out,
                "\n  SEPARATION DEEPENS THE BATCH. The two sides decouple: one runs"
            );
            let _ = writeln!(
                out,
                "  ahead, a real backlog forms, and each shared read is amortised"
            );
            let _ = writeln!(
                out,
                "  over it. That is the condition peer-index caching needs, and it"
            );
            let _ = writeln!(
                out,
                "  is a property of PLACEMENT -- not of the architecture."
            );
        } else if same > cross * 2.0 {
            let _ = writeln!(
                out,
                "\n  THE HYPOTHESIS IS REFUTED, AND BACKWARDS. Threads placed"
            );
            let _ = writeln!(
                out,
                "  TOGETHER batch {:.0}x deeper than threads placed apart, where the",
                same / cross.max(0.001)
            );
            let _ = writeln!(
                out,
                "  prediction was the reverse -- that mismatched cores would"
            );
            let _ = writeln!(out, "  decouple and batch deeply.");
            let _ = writeln!(
                out,
                "  A coherent reading: a cheap handoff lets the producer race ahead"
            );
            let _ = writeln!(
                out,
                "  and build a backlog, while an expensive one throttles it into"
            );
            let _ = writeln!(
                out,
                "  lockstep, so each side arrives to find exactly one item. Cost"
            );
            let _ = writeln!(
                out,
                "  drives depth, rather than depth being set by core speed."
            );
            let _ = writeln!(
                out,
                "  That is a hypothesis this run does not test, and it must not be"
            );
            let _ = writeln!(
                out,
                "  recorded as a finding -- what IS established is that the"
            );
            let _ = writeln!(out, "  original prediction is wrong.");
        } else {
            let _ = writeln!(
                out,
                "\n  Placement does NOT move batch depth here ({:.2}x).",
                cross / same
            );
            let _ = writeln!(
                out,
                "  The hypothesis that unequal core speeds drive the batching is"
            );
            let _ = writeln!(
                out,
                "  not supported, and the difference between hosts needs another"
            );
            let _ = writeln!(
                out,
                "  explanation. Recording a refutation is the point of running it."
            );
        }
    }

    // The plainest answer to "does placement matter", independent of caching.
    // "Near" falls back to SMT siblings, because a host whose outermost
    // partitioning cache is per-core has no same-cache-different-core pair at
    // all -- its nearest expressible placement IS the sibling pair.
    if let (Some(near), Some(far)) = (
        observation
            .get(Placement::SameCacheSameClass, Strategy::Baseline)
            .or_else(|| observation.get(Placement::SameCoreSiblings, Strategy::Baseline)),
        observation
            .get(Placement::CrossCacheCrossClass, Strategy::Baseline)
            .or_else(|| observation.get(Placement::CrossCacheSameClass, Strategy::Baseline)),
    ) {
        let _ = writeln!(
            out,
            "\n  the unoptimised handoff costs {:.1} ns/item together and {:.1} ns/item",
            near.nanos_per_item, far.nanos_per_item
        );
        let _ = writeln!(
            out,
            "  apart -- {:.1}x for crossing the boundary, with no code change.",
            far.nanos_per_item / near.nanos_per_item
        );
    }

    let _ = writeln!(
        out,
        "\n  does the verdict on caching depend on placement?\n"
    );
    let mut verdicts = Vec::new();
    for placement in expressible {
        let (Some(base), Some(cached)) = (
            observation.get(placement, Strategy::Baseline),
            observation.get(placement, Strategy::Cached),
        ) else {
            continue;
        };
        let speedup = base.nanos_per_item / cached.nanos_per_item;
        let verdict = if speedup >= 1.1 {
            "caching WINS"
        } else if speedup <= 0.9 {
            "caching LOSES"
        } else {
            "no effect"
        };
        let _ = writeln!(
            out,
            "    {:<26} {:>7.2}x   {verdict}",
            placement.label(),
            speedup
        );
        verdicts.push(verdict);
    }
    verdicts.sort_unstable();
    verdicts.dedup();

    if verdicts.len() > 1 {
        let _ = writeln!(
            out,
            "\n  THE VERDICT FLIPS WITHIN ONE MACHINE. A technique whose sign"
        );
        let _ = writeln!(
            out,
            "  depends on where two threads are scheduled cannot be adopted or"
        );
        let _ = writeln!(
            out,
            "  rejected by a fixed decision. Any answer has to name the"
        );
        let _ = writeln!(out, "  placement it holds for.");
    } else {
        let _ = writeln!(
            out,
            "\n  The verdict is the same at every placement on this host, so"
        );
        let _ = writeln!(
            out,
            "  placement alone does not explain the disagreement between hosts."
        );
    }

    out
}

/// Print the per-node-pair handoff cost, when the host has nodes to cross.
///
/// Silent on a single-node machine: there is nothing to say, and a header over
/// an empty table invites the reader to wonder what went wrong.
fn render_node_distances(out: &mut String, observation: &Observation) {
    let pairs = observation.node_pairs_measured();
    if pairs.is_empty() {
        return;
    }

    let _ = writeln!(out, "\n-- the handoff, by NUMA node pair --");
    // A ring-placement column, because a pair and a strategy no longer identify
    // one row: every hop is measured once with the ring on the producer's node
    // and once on the consumer's. Rendering one of them would drop half the
    // measurements and, worse, could pair a baseline taken at one placement
    // against a cached run taken at the other.
    let _ = writeln!(
        out,
        "{:<14} {:>8} {:>8} {:>8} {:>12} {:>12} {:>10}",
        "prod -> cons", "ring on", "prod", "cons", "base ns/it", "cached ns/it", "cach depth"
    );
    // Stated rather than left as a mystery glyph. `ring on` names the node the
    // run asked for, since that is what identifies the row; a `!` means the
    // memory did not land there, so that row does not measure the placement it
    // names.
    let _ = writeln!(
        out,
        "  (`ring on` is the node requested; `!` means it landed elsewhere)"
    );

    let mut slowest: Option<(f64, (u32, u32))> = None;
    let mut fastest: Option<(f64, (u32, u32))> = None;

    for pair in &pairs {
        for base in observation.node_pair_rows(*pair, Strategy::Baseline) {
            // Matched on the ring placement as well, so the two columns
            // describe the same configuration -- and on the placement that was
            // *requested*, not the one that was achieved. Windows may redirect
            // an allocation, so two rows can share an achieved node while
            // describing different placements; keyed on that, this pairs a
            // baseline taken at one placement against a cached run taken at the
            // other, which is the exact error the comment above says the key
            // exists to prevent.
            let Some(cached) =
                observation.node_pair(*pair, Strategy::Cached, base.requested_memory_node)
            else {
                continue;
            };
            let _ = writeln!(
                out,
                "{:<14} {:>8} {:>8} {:>8} {:>12.1} {:>12.1} {:>10.1}",
                // `->`, not `<->`: hops are directed, because the producer
                // writes and the consumer reads. The probe crate's own report
                // was corrected for this and this second renderer of the same
                // data was not, which is how two views of one measurement drift
                // apart.
                format!("{} -> {}", pair.0, pair.1),
                // The requested node, matching the key above and the probe
                // crate's own report. A trailing `!` marks a row whose memory
                // did not land where it was asked to go, so a redirected run
                // is not read as a measurement of the placement it names.
                match (base.requested_memory_node, base.memory_node) {
                    (Some(asked), Some(got)) if asked == got => format!("node {asked}"),
                    (Some(asked), _) => format!("node {asked}!"),
                    (None, _) => "unspecified".to_owned(),
                },
                format!("g{}/cpu{}", base.producer.group, base.producer.number),
                format!("g{}/cpu{}", base.consumer.group, base.consumer.number),
                base.nanos_per_item,
                cached.nanos_per_item,
                cached.consumer_batch
            );
            let seen = (base.nanos_per_item, *pair);
            if slowest.is_none_or(|(worst, _)| seen.0 > worst) {
                slowest = Some(seen);
            }
            if fastest.is_none_or(|(best, _)| seen.0 < best) {
                fastest = Some(seen);
            }
        }
    }

    if pairs.len() == 1 {
        let _ = writeln!(
            out,
            "\n  One node pair, so this restates the `cross NUMA node` row above\n  \
             rather than adding to it. The table earns its place from three\n  \
             nodes upward, where the hops stop being interchangeable."
        );
        return;
    }

    let (Some((worst, worst_pair)), Some((best, best_pair))) = (slowest, fastest) else {
        return;
    };
    let _ = writeln!(
        out,
        "\n  {} node pairs. Cheapest hop {} <-> {} at {:.1} ns/item; dearest\n  \
         {} <-> {} at {:.1} ns/item -- a spread of {:.1}x.",
        pairs.len(),
        best_pair.0,
        best_pair.1,
        best,
        worst_pair.0,
        worst_pair.1,
        worst,
        worst / best
    );
    if worst / best < 1.2 {
        let _ = writeln!(
            out,
            "  That spread is small enough that this host's nodes are close to\n  \
             equidistant, so the single `cross NUMA node` row above is a fair\n  \
             summary of it."
        );
    } else {
        let _ = writeln!(
            out,
            "  The hops are NOT interchangeable, so the single `cross NUMA node`\n  \
             row above reports whichever one was enumerated first and should not\n  \
             be read as 'the' cost of leaving a node."
        );
    }
    let _ = writeln!(
        out,
        "  This measures the handoff between two nodes; it is not a distance\n  \
         matrix read from firmware. Windows exposes no NUMA distance table, so\n  \
         these numbers are the observable rather than a restatement of ACPI."
    );
}
