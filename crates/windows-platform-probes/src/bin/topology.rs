// Copyright (c) Mike Grier.

//! Prints the machine's processor topology, and how many execution domains each
//! candidate partitioning policy would produce on it.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! Running this on every CI build is deliberate: hosted runners are a
//! heterogeneous fleet, so the accumulated output is a slow survey of what real
//! machines look like. The line tagged `x-probe-topology` is emitted as a single
//! JSON object so those results can be mined out of build logs mechanically
//! rather than read by eye.

use std::fmt::Write as _;
use windows_platform_probes::report::{Stdout, emit};
use windows_platform_probes::topology::measure;

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(&mut Stdout, &render());
}

/// The probe's whole report, as text.
fn render() -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "== processor topology, and what each partitioning policy would yield ==\n"
    );

    let observation = match measure() {
        Ok(observation) => observation,
        Err(error) => {
            let _ = writeln!(out, "Topology::discover failed: {error}");
            let _ = writeln!(
                out,
                "(Reported rather than measured: a probe that cannot read its"
            );
            let _ = writeln!(
                out,
                "subject must say so instead of printing a misleading shape.)"
            );
            return out;
        }
    };

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
        observation.numa_domains, observation.memoryless_numa_domains
    );
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
    }

    let _ = writeln!(out, "\ncaches:");
    if observation.caches.is_empty() {
        let _ = writeln!(out, "  none reported");
    }
    for cache in &observation.caches {
        let _ = writeln!(
            out,
            "  L{:<2} {:>3} domain(s), processors per domain: {:?}",
            cache.level, cache.domains, cache.processors_per_domain
        );
    }

    match observation.outermost_partitioning_cache() {
        Some(cache) => {
            let _ = writeln!(
                out,
                "\noutermost cache that partitions this machine: L{} ({} domains)",
                cache.level, cache.domains
            );
        }
        None => {
            let _ = writeln!(
                out,
                "\nno cache level partitions this machine: every level is machine-wide"
            );
        }
    }
    if !observation.caches.iter().any(|c| c.level == 3) {
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
            let _ = writeln!(
                out,
                "  GetNumaHighestNodeNumber    : {highest} (so {} nodes)",
                highest + 1
            );
        }
        None => {
            let _ = writeln!(out, "  GetNumaHighestNodeNumber    : failed");
        }
    }
    let complaints = observation.cross_check();
    if complaints.is_empty() {
        let _ = writeln!(
            out,
            "  => agree. windows-topology-sys parsed this machine consistently."
        );
    } else {
        let _ = writeln!(out, "  => DISAGREE. This is a finding, not a nuisance:");
        for complaint in &complaints {
            let _ = writeln!(out, "     - {complaint}");
        }
    }

    // One machine-readable line, so accumulated CI logs can be mined without
    // parsing the prose above. Kept to a single line on purpose.
    let cache_json: Vec<String> = observation
        .caches
        .iter()
        .map(|c| format!(r#"{{"level":{},"domains":{}}}"#, c.level, c.domains))
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
            r#""packages":{},"numa_domains":{},"memoryless_numa_domains":{},"cores":{},"#,
            r#""efficiency_classes":{},"caches":[{}],"outermost_partitioning_cache_level":{},"#,
            r#""policies":{{{}}},"cross_check_ok":{}}}"#
        ),
        std::env::consts::ARCH,
        observation.online_processors,
        observation.groups,
        observation.packages,
        observation.numa_domains,
        observation.memoryless_numa_domains,
        observation.cores.len(),
        classes.len(),
        cache_json.join(","),
        observation
            .outermost_partitioning_cache()
            .map_or("null".to_string(), |c| c.level.to_string()),
        policy_json.join(","),
        complaints.is_empty(),
    );
    out
}
