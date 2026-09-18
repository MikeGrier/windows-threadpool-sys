# Checklist: windows-topology-sys

The `MMT-*` plan -- the MachineMemoryTopology reshape that gated PR #56 -- is complete and archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) under `Moved 2026-09-03`, together with the M1-M4
enumeration plan that preceded it. Cite item IDs (`MMT-1.1`, `M4+.1`, `M5+.4`, ...) against that file.

Decisions live in [DESIGN-NOTES.md](DESIGN-NOTES.md), which is the authority for current behaviour;
the archived checklist records what was *done*, not what is *true now*.

## Deferred, and why

Two things were deliberately left out of the reshape rather than forgotten:

- **CPU-set flag bit positions** ([D-23](DESIGN-NOTES.md#d-23)). `SYSTEM_CPU_SET_INFORMATION::AllFlags`
  reads constant zero on this build, *even after* `SetProcessDefaultCpuSets` succeeds and
  `GetProcessDefaultCpuSets` confirms the allocation. The bit positions are therefore neither
  confirmed nor falsifiable here; verification needs a machine that populates the byte. This is a
  blocked measurement, not an unwritten one.

- **The planner adapters.** Per [D-21](DESIGN-NOTES.md#d-21) this crate is the refined view of what the
  platform publishes and is self-justified as such; the adapter onto
  [topology-planner](../topology-planner/CHECKLIST.md)'s traits belongs on the planner's side of the
  boundary, and is planned there.
