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

use windows_platform_probes::topology::measure;

fn main() {
    println!("== processor topology, and what each partitioning policy would yield ==\n");

    let observation = match measure() {
        Ok(observation) => observation,
        Err(error) => {
            println!("Topology::discover failed: {error}");
            println!("(Reported rather than measured: a probe that cannot read its");
            println!("subject must say so instead of printing a misleading shape.)");
            return;
        }
    };

    println!("processors (online) : {}", observation.online_processors);
    println!("processor groups    : {}", observation.groups);
    println!("packages            : {}", observation.packages);
    println!(
        "NUMA domains        : {} ({} with no processors)",
        observation.numa_domains, observation.memoryless_numa_domains
    );
    println!("physical cores      : {}", observation.cores.len());

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
    println!("  cores with SMT    : {smt}");
    println!("  efficiency classes: {classes:?}");
    if classes.len() > 1 {
        println!("  (heterogeneous: an I/O thread left unconstrained can land on an");
        println!("   efficiency core, which is why even a single domain wants a mask)");
    }

    println!("\ncaches:");
    if observation.caches.is_empty() {
        println!("  none reported");
    }
    for cache in &observation.caches {
        println!(
            "  L{:<2} {:>3} domain(s), processors per domain: {:?}",
            cache.level, cache.domains, cache.processors_per_domain
        );
    }

    match observation.outermost_partitioning_cache() {
        Some(cache) => println!(
            "\noutermost cache that partitions this machine: L{} ({} domains)",
            cache.level, cache.domains
        ),
        None => println!("\nno cache level partitions this machine: every level is machine-wide"),
    }
    if !observation.caches.iter().any(|c| c.level == 3) {
        println!("NOTE: this machine reports no L3 at all, so a policy keyed literally");
        println!("on \"L3\" would find nothing here. That is the measured case behind");
        println!("phrasing the rule as \"the outermost level that partitions\".");
    }

    println!("\ndomains each policy would produce:");
    for (name, count) in observation.domain_counts() {
        println!("  {name:<34} {count}");
    }

    println!("\ncross-check against independently read Win32 counters:");
    println!(
        "  GetActiveProcessorCount     : {}",
        observation.raw_active_processors
    );
    println!(
        "  GetActiveProcessorGroupCount: {}",
        observation.raw_group_count
    );
    match observation.raw_highest_numa_node {
        Some(highest) => println!(
            "  GetNumaHighestNodeNumber    : {highest} (so {} nodes)",
            highest + 1
        ),
        None => println!("  GetNumaHighestNodeNumber    : failed"),
    }
    let complaints = observation.cross_check();
    if complaints.is_empty() {
        println!("  => agree. windows-topology-sys parsed this machine consistently.");
    } else {
        println!("  => DISAGREE. This is a finding, not a nuisance:");
        for complaint in &complaints {
            println!("     - {complaint}");
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
    println!(
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
}
