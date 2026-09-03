# windows-topology-sys

Safe enumeration of Windows processor, cache, and memory topology.

**Windows only.** Every item is behind `cfg(windows)`; the crate builds to an
empty shell on other platforms.

## Example

```rust,no_run
use windows_topology_sys::MachineMemoryTopology;

let topology = MachineMemoryTopology::discover()?;
println!(
    "{} logical processor(s), {} domain(s)",
    topology.processors.len(),
    topology.domains.len()
);
# Ok::<(), std::io::Error>(())
```

[`examples/print_topology.rs`](examples/print_topology.rs) prints the host's
full topology as JSON (`cargo run --example print_topology --features serde`),
in the same shape a hand-written or fed-in description takes.

## Why this crate exists

`GetLogicalProcessorInformationEx` is the Win32 entry point for topology, and
the `windows` crate exposes it as typed but still `unsafe` FFI: a raw output
pointer, two-call sizing, and records that must be walked by their own
self-reported `Size` rather than indexed as a slice. Several of the records
also declare a trailing array as length 1 (`PROCESSOR_RELATIONSHIP::GroupMask:
[GROUP_AFFINITY; 1]`) while actually holding as many entries as `GroupCount`
reports -- reading past element 0 is exactly what correct use of the API
requires, and exactly what Rust calls undefined behavior if done through the
declared type. None of that is solved by a thin `unsafe fn` wrapper; it has to
be solved by walking the buffer correctly.

This crate does that walk once, safely, and hands back owned records.

## Scope

**What this is:** safe enumeration ([`MachineMemoryTopology::discover`]), plus a plain-data
description ([`MachineMemoryTopology`], [`Domain`]) that needs no Windows API to construct
-- build one by hand, or (with the `serde` feature) deserialize one from JSON
written for a machine you do not have.

**What this is not:** an opinionated topology model. It does not decide what
counts as a "locality domain worth partitioning by" -- by NUMA node, by
last-level cache, by package -- that is the consumer's call, because the
right answer depends on the workload. It is also not a partitioning policy,
and not a device topology: no NVMe controller, NIC, or GPU is a topology
participant here, and there is no HMAT-style attributed-distance model. Both
were considered and declined for now.

See [DESIGN-NOTES.md](DESIGN-NOTES.md) for the full reasoning, including a
cross-check against Linux's topology model, D-9's full list of what was
declined and why, and D-8's note that the JSON schema is not covered by this
crate's semver contract.

Run `cargo run --example print_topology --features serde` to see the host's
own topology as JSON -- the shape a hand-written or synthetic description
takes.

[`MachineMemoryTopology::discover`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.MachineMemoryTopology.html#method.discover
[`MachineMemoryTopology`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.MachineMemoryTopology.html
[`Domain`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.Domain.html

## License

MIT. Copyright (c) Mike Grier.
