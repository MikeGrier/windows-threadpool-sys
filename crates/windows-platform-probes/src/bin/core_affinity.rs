// Copyright (c) Mike Grier.

//! Prints whether it matters where the two ends of a queue run.

use windows_platform_probes::core_affinity::{Placement, measure};
use windows_platform_probes::peer_index_cache::Strategy;

fn main() -> std::io::Result<()> {
    windows_platform_probes::fingerprint::print_banner();
    println!("== does it matter where the two ends of a queue run? ==\n");

    let observation = measure()?;

    println!("processors, as discovered:");
    println!(
        "  {:>4}  {:>16}  {:>13}",
        "cpu", "efficiency class", "cache domain"
    );
    for place in &observation.processors {
        println!(
            "  {:>4}  {:>16}  {:>13}",
            place.number,
            place.efficiency_class,
            place
                .cache_domain
                .map_or_else(|| "none".to_owned(), |id| id.to_string())
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
    println!(
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
        println!("\n-- the same handoff, within each efficiency class --");
        println!(
            "{:<12} {:>4} {:>4} {:>12} {:>12} {:>10}",
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
                println!(
                    "{:<12} {:>4} {:>4} {:>12.1} {:>12.1} {:>10.1}",
                    format!("class {class}"),
                    base.producer.number,
                    base.consumer.number,
                    base.nanos_per_item,
                    cached.nanos_per_item,
                    cached.consumer_batch
                );
            }
        }
        println!(
            "  (Windows numbers efficiency classes with the FASTER cores higher, so\n   \
             the highest class here is the performance one.)"
        );
    }

    println!("\n-- the handoff, by placement --");
    println!(
        "{:<26} {:>4} {:>4} {:>12} {:>12} {:>10} {:>10}",
        "placement", "prod", "cons", "base ns/it", "cached ns/it", "base depth", "cach depth"
    );

    let all = [
        Placement::SameCacheSameClass,
        Placement::SameCacheCrossClass,
        Placement::CrossCacheSameClass,
        Placement::CrossCacheCrossClass,
    ];

    for placement in all {
        let (Some(base), Some(cached)) = (
            observation.get(placement, Strategy::Baseline),
            observation.get(placement, Strategy::Cached),
        ) else {
            // Absent is a finding, not a gap: it means this machine cannot
            // express the placement at all.
            println!(
                "{:<26} {:>4} {:>4} {:>12} {:>12} {:>10} {:>10}",
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
        println!(
            "{:<26} {:>4} {:>4} {:>12.1} {:>12.1} {:>10.1} {:>10.1}",
            placement.label(),
            base.producer.number,
            base.consumer.number,
            base.nanos_per_item,
            cached.nanos_per_item,
            base.consumer_batch,
            cached.consumer_batch
        );
    }

    println!("\nthe slice each row was measured on:");
    for placement in all {
        if let Some(base) = observation.get(placement, Strategy::Baseline) {
            println!("  {:<26} {}", placement.label(), base.slice);
        }
    }

    println!("\ninterpretation:\n");

    let expressible = observation.placements();
    if expressible.len() < 2 {
        println!("  This machine expresses only one placement, so it cannot answer");
        println!("  the question. That is a fact about the host, not a null result:");
        println!("  a homogeneous single-cache machine has nowhere else to put the");
        println!("  two threads.");
        return Ok(());
    }

    // Whether the two factors can be told apart at all on this host. If every
    // cross-class pair is also cross-cache, they are perfectly confounded and
    // no amount of measurement here separates them -- which is a fact to state,
    // not to reason past.
    let confounded = !expressible.contains(&Placement::SameCacheCrossClass)
        && !expressible.contains(&Placement::CrossCacheSameClass);
    if confounded {
        println!("  CAUTION: on this machine the efficiency classes and the cache");
        println!("  domains coincide exactly, so every cross-class pair is also a");
        println!("  cross-cache pair. The two effects are perfectly CONFOUNDED here");
        println!("  and nothing below separates them. Read the rows as 'within a");
        println!("  domain' versus 'across domains', and do not attribute the");
        println!("  difference to core speed or to cache without a machine whose");
        println!("  classes and caches cut differently.\n");
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
            |m: &windows_platform_probes::core_affinity::Measurement| m.consumer_batch,
        ),
        mean(
            &cross_class,
            |m: &windows_platform_probes::core_affinity::Measurement| m.consumer_batch,
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
        println!("  batch depth with caching on, {within}: {same:.1} items per shared read");
        println!("  batch depth with caching on, {across}: {cross:.1} items per shared read");
        if cross > same * 2.0 {
            println!("\n  SEPARATION DEEPENS THE BATCH. The two sides decouple: one runs");
            println!("  ahead, a real backlog forms, and each shared read is amortised");
            println!("  over it. That is the condition peer-index caching needs, and it");
            println!("  is a property of PLACEMENT -- not of the architecture.");
        } else if same > cross * 2.0 {
            println!("\n  THE HYPOTHESIS IS REFUTED, AND BACKWARDS. Threads placed");
            println!(
                "  TOGETHER batch {:.0}x deeper than threads placed apart, where the",
                same / cross.max(0.001)
            );
            println!("  prediction was the reverse -- that mismatched cores would");
            println!("  decouple and batch deeply.");
            println!("  A coherent reading: a cheap handoff lets the producer race ahead");
            println!("  and build a backlog, while an expensive one throttles it into");
            println!("  lockstep, so each side arrives to find exactly one item. Cost");
            println!("  drives depth, rather than depth being set by core speed.");
            println!("  That is a hypothesis this run does not test, and it must not be");
            println!("  recorded as a finding -- what IS established is that the");
            println!("  original prediction is wrong.");
        } else {
            println!(
                "\n  Placement does NOT move batch depth here ({:.2}x).",
                cross / same
            );
            println!("  The hypothesis that unequal core speeds drive the batching is");
            println!("  not supported, and the difference between hosts needs another");
            println!("  explanation. Recording a refutation is the point of running it.");
        }
    }

    // The plainest answer to "does placement matter", independent of caching.
    if let (Some(near), Some(far)) = (
        observation.get(Placement::SameCacheSameClass, Strategy::Baseline),
        observation
            .get(Placement::CrossCacheCrossClass, Strategy::Baseline)
            .or_else(|| observation.get(Placement::CrossCacheSameClass, Strategy::Baseline)),
    ) {
        println!(
            "\n  the unoptimised handoff costs {:.1} ns/item together and {:.1} ns/item",
            near.nanos_per_item, far.nanos_per_item
        );
        println!(
            "  apart -- {:.1}x for crossing the boundary, with no code change.",
            far.nanos_per_item / near.nanos_per_item
        );
    }

    println!("\n  does the verdict on caching depend on placement?\n");
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
        println!(
            "    {:<26} {:>7.2}x   {verdict}",
            placement.label(),
            speedup
        );
        verdicts.push(verdict);
    }
    verdicts.sort_unstable();
    verdicts.dedup();

    if verdicts.len() > 1 {
        println!("\n  THE VERDICT FLIPS WITHIN ONE MACHINE. A technique whose sign");
        println!("  depends on where two threads are scheduled cannot be adopted or");
        println!("  rejected by a fixed decision. Any answer has to name the");
        println!("  placement it holds for.");
    } else {
        println!("\n  The verdict is the same at every placement on this host, so");
        println!("  placement alone does not explain the disagreement between hosts.");
    }

    Ok(())
}
