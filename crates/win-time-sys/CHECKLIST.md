# Checklist: win-time-sys

Safe Rust over the Windows clocks. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.
WT-M1, the crate and its clocks, is complete and archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## WT-M2 -- Adoption

- [x] **WT-2.1** -- `windows-threadpool-sys` reads interrupt time through `InterruptClock`, so the workspace has one reader of its one time base. -> [completed 2026-10-08](COMPLETED-CHECKLIST.md#wt-21)

- [ ] **WT-2.2** -- **durable-ioring's timestamps use `InterruptClock`**
  ([DI-D-37](../durable-ioring/DESIGN-NOTES.md#di-d-37)).

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M3` ->
  > `DI-3.2.4.2` (nullifiers, whose markings and `Failed` entries are timestamped). See
  > [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

## M-inf -- Parked

- [ ] **WT-inf.1** -- **A cycle-counter clock** (`rdtscp` on x64; the architecture's virtual counter on
  ARM64, which needs `asm!`). Deferred by the engineer ([WT-D-6](DESIGN-NOTES.md#wt-d-6)), blocked on
  having no mapping from cycles to real-world time.
