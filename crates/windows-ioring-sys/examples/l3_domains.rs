// Copyright (c) 2026 Mike Grier
//! M6.3: enumerate last-level-cache (L3) domains -- the default heuristic
//! `DESIGN-NOTES.md`'s "Why the NUMA node is the wrong key" recommends for
//! sizing an `IoRing` execution domain. This is enumeration only, not a
//! partitioning policy: what to do with the domains is a workload call this
//! crate deliberately leaves to the caller (see `windows-topology-sys` for a
//! safe `GetLogicalProcessorInformationEx` wrapper).

fn main() -> std::io::Result<()> {
    let relations = windows_topology_sys::discover()?;
    let l3_domains: Vec<_> = relations
        .caches
        .iter()
        .filter(|cache| cache.level == 3)
        .collect();

    println!("{} last-level cache (L3) domain(s):", l3_domains.len());
    for (index, domain) in l3_domains.iter().enumerate() {
        println!(
            "  domain {index}: {} bytes shared by {:?}",
            domain.cache_size, domain.processors
        );
    }

    // Processor groups are a hard floor (D-8 in DESIGN-NOTES.md): above 64
    // logical processors, a thread's affinity and a ring's waiter are each
    // confined to one GROUP_AFFINITY, whether or not that partition is
    // wanted.
    println!("{} processor group(s)", relations.groups.len());

    Ok(())
}
