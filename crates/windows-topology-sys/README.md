# windows-topology-sys

A refined view of the processor, cache, and memory topology Windows publishes.

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

## What the model is

**Observed connectivity, not a ladder of levels with optional rungs.** The
difference matters to a consumer: a level-shaped model has to invent a value
for a rung the platform did not report, and the invented value is
indistinguishable from a measured one.

- **Relations are held as a set, and never reduced on insert.** Windows
  describes processors through *two* APIs --
  `GetLogicalProcessorInformationEx` and `GetSystemCpuSetInformation` -- and
  they are not the same API twice. Each relation carries an [`Observation`]
  naming its [`Source`], so where the two disagree the disagreement survives
  rather than being silently resolved in favour of whichever was read last.
  [`MachineMemoryTopology::cpu_sets`] also keeps the CPU-set view verbatim.
- **[`Observed<T>`] distinguishes three facts that a plain `Option` collapses
  into two**: `Known`, `Absent` ("asked, and there is none"), and `NotObserved`
  ("nobody asked"). With the `serde` feature these are three distinct
  encodings, so a description round-trips the distinction instead of losing it.
- **[`Granularity`] orders the domain kinds**, and `minimal_shared` is the meet
  -- so "how close are these two processors?" is answered over every kind and
  any cache depth, rather than over one nominated level. `proximity` derives
  the pairwise answer from an inclusion-ordered partitioning rather than
  restating the rule.
- **[`MachineMemoryTopology::enumeration_anomalies`] records what could not be
  decoded.** Empty on every healthy machine. A malformed record is neither a
  panic nor a silent truncation, so a short list is distinguishable from a
  small machine.
- **[`Provenance`] records how the object was obtained** -- discovered,
  restored from a description, or hand-built -- which is a fact about the
  construction, orthogonal to which source reported any given relation.

## Scope

**What this is:** safe enumeration ([`MachineMemoryTopology::discover`]), plus a plain-data
description ([`MachineMemoryTopology`], [`Domain`]) that needs no Windows API to construct
-- build one by hand, or (with the `serde` feature) deserialize one from JSON
written for a machine you do not have.

**What this is not:** a partitioning policy. It answers what the machine looks
like, never which boundary you should shard on -- that depends on the workload,
so it is the consumer's call. It is also not a device topology: no NVMe
controller, NIC, or GPU is a participant here.

**It does not go below the Win32 topology APIs.** If Windows does not report a
fact, this crate does not have it. That is a scope boundary rather than a
judgement about the fact: ACPI carries SLIT distances, no Win32 API surfaces
them, and reading firmware directly would be going below the boundary -- so
there is no attributed-distance model, and none is planned here.

See [DESIGN-NOTES.md](DESIGN-NOTES.md) for the full reasoning, including a
cross-check against Linux's topology model, D-9's list of what was declined and
why, D-20's scope ruling above, and D-8's note that the JSON schema is not
covered by this crate's semver contract.

[`MachineMemoryTopology::discover`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.MachineMemoryTopology.html#method.discover
[`MachineMemoryTopology`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.MachineMemoryTopology.html
[`MachineMemoryTopology::cpu_sets`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.MachineMemoryTopology.html#structfield.cpu_sets
[`MachineMemoryTopology::enumeration_anomalies`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.MachineMemoryTopology.html#structfield.enumeration_anomalies
[`Domain`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.Domain.html
[`Observation`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/struct.Observation.html
[`Source`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/enum.Source.html
[`Observed<T>`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/enum.Observed.html
[`Granularity`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/enum.Granularity.html
[`Provenance`]: https://docs.rs/windows-topology-sys/latest/windows_topology_sys/enum.Provenance.html

## License

MIT. Copyright (c) Mike Grier.
