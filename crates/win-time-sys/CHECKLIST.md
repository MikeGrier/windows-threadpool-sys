# Checklist: win-time-sys

Safe Rust over the Windows clocks. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.
WT-M1, the crate and its clocks, is complete and archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## WT-M2 -- Adoption

- [x] **WT-2.1** -- `windows-threadpool-sys` reads interrupt time through `InterruptClock`, so the workspace has one reader of its one time base. -> [completed 2026-10-08](COMPLETED-CHECKLIST.md#wt-21)

- [x] **WT-2.2.1** -- `Steady`, the marker trait for a clock whose readings never decrease, made by the interrupt and performance clocks and not by the system clocks. -> [completed 2026-10-08](COMPLETED-CHECKLIST.md#wt-221)

- [ ] **WT-2.2.2** -- **durable-ioring's timestamps use a `Steady` interrupt-time clock,
  `InterruptClock` by default** ([DI-D-37](../durable-ioring/DESIGN-NOTES.md#di-d-37),
  [DI-D-38](../durable-ioring/DESIGN-NOTES.md#di-d-38)): dioring depends on this crate; its core
  and `Dioring` are generic over the clock, with a constructor that takes one; and its first
  timestamp -- when a failure was observed, carried by its `Failed` entry and by the inventory
  ([DI-D-36](../durable-ioring/DESIGN-NOTES.md#di-d-36)) -- is read from it. The oracle checks that
  failure stamps never decrease in queue order, and CONTRACT.md states both time facts. The marking
  timestamps `DI-3.2.4.2` adds use the same clock.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M3` ->
  > `DI-3.2.4.2` (nullifiers, whose markings and `Failed` entries are timestamped). See
  > [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

## M-inf -- Parked

- [ ] **WT-inf.1** -- **A cycle-counter clock** (`rdtscp` on x64; the architecture's virtual counter on
  ARM64, which needs `asm!`). Deferred by the engineer ([WT-D-6](DESIGN-NOTES.md#wt-d-6)), blocked on
  having no mapping from cycles to real-world time.

- [ ] **WT-inf.2** -- **A clock that colludes with a timed wait**, so a mock can make a long wait pass
  quickly with the code waiting none the wiser (the working position in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#working-position-mock-clocks-and-timed-waits-not-a-decision)).
  Gated on the first timed wait that accounts time through a clock: its affordance depends on that
  wait's shape.
