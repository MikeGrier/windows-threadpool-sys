# Checklist: win-time-sys

Safe Rust over the Windows clocks. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.
WT-M1, the crate and its clocks, and WT-M2, its adoption by `windows-threadpool-sys` and
durable-ioring, are complete and archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M-inf -- Parked

- [ ] **WT-inf.1** -- **A cycle-counter clock** (`rdtscp` on x64; the architecture's virtual counter on
  ARM64, which needs `asm!`). Deferred by the engineer ([WT-D-6](DESIGN-NOTES.md#wt-d-6)), blocked on
  having no mapping from cycles to real-world time.

- [ ] **WT-inf.2** -- **A clock that colludes with a timed wait**, so a mock can make a long wait pass
  quickly with the code waiting none the wiser (the working position in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#working-position-mock-clocks-and-timed-waits-not-a-decision)).
  Gated on the first timed wait that accounts time through a clock: its affordance depends on that
  wait's shape.
