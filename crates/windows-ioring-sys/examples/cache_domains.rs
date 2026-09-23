// Copyright (c) 2026 Mike Grier
// Renamed from examples/l3_domains.rs at 8b8afaf6. The rewrite left the files
// 17% similar, far under git's rename threshold, so blame and log do not
// follow it without help.
//! M6.3: enumerate the **outermost cache level that actually partitions this
//! machine** -- the default heuristic `DESIGN-NOTES.md`'s "Why the NUMA node
//! is the wrong key" recommends for sizing an `IoRing` execution domain.
//!
//! This is enumeration only, not a partitioning policy: what to do with the
//! domains is a workload call this crate deliberately leaves to the caller
//! (see `windows-topology-sys` for a safe `GetLogicalProcessorInformationEx`
//! wrapper, and `examples/ring_copy` for a sample that does make the call).
//!
//! # Why this is no longer `l3_domains.rs`
//!
//! It filtered `cache.level == 3` and called the result "last-level cache",
//! which is two assumptions wearing one name: that the machine *has* an L3,
//! and that level numbering orders caches from inner to outer. A shipping
//! Snapdragon X2 Elite falsifies the first -- no L3 at all, with the natural
//! cluster boundary at L2 ([D-48](../DESIGN-NOTES.md#d-48)) -- so the old
//! version reported "0 last-level cache domain(s)" on a 12-core machine that
//! plainly has cache domains.
//!
//! `MachineMemoryTopology::outermost_partitioning_cache` owns the rule now,
//! including the part this file could never have got right by filtering: a
//! level qualifies only when its blocks are **pairwise disjoint**, and
//! "outermost" is decided by inclusion rather than by the level number.

use std::io::Write;

use windows_topology_sys::MachineMemoryTopology;

fn main() -> std::io::Result<()> {
    // The single sink every line of this sample's output goes through
    // (repository "Architectural pre-steps" rule: never call `println!`
    // from more than one call site).
    let mut out = std::io::stdout();
    let topology = MachineMemoryTopology::discover()?;

    match topology.outermost_partitioning_cache() {
        Some((level, domains)) => {
            let _ = writeln!(
                out,
                "{} cache domain(s) at L{level}, the outermost level that partitions this machine:",
                domains.len()
            );
            for (index, domain) in domains.iter().enumerate() {
                let _ = writeln!(out, "  domain {index}: {:?}", domain.processors);
            }
        }
        // Not an error, and not an empty list dressed up as one. It is a
        // statement about the machine: no cache level splits it into more
        // than one disjoint block, so caches offer no partition to size a
        // ring by. One ring is the correct answer here, which is the same
        // degradation `ring_copy`'s `Policy::select` reports as `degraded`.
        None => {
            let _ = writeln!(
                out,
                "no cache level partitions this machine, so caches offer no domain \
                 boundary to size a ring by; one ring is correct here"
            );
        }
    }

    // Processor groups are a hard floor (D-8 in DESIGN-NOTES.md): above 64
    // logical processors, a thread's affinity and a ring's waiter are each
    // confined to one GROUP_AFFINITY, whether or not that partition is
    // wanted.
    let groups: std::collections::BTreeSet<u16> =
        topology.processors.iter().map(|p| p.id.group).collect();
    let _ = writeln!(out, "{} processor group(s)", groups.len());

    Ok(())
}
