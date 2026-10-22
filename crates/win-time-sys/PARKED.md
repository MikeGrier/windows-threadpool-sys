# Parked: win-time-sys

Work not in scope now. Nothing here is pending; an item leaves only by graduating into a milestone of
[CHECKLIST.md](CHECKLIST.md), or by a ruling that withdraws it.

## M-inf -- Parked

- [ ] **WT-inf.1** -- **A cycle-counter clock** (`rdtscp` on x64; the architecture's virtual counter on
  ARM64, which needs `asm!`). Deferred by the engineer ([WT-D-6](DESIGN-NOTES.md#wt-d-6)), blocked on
  having no mapping from cycles to real-world time.

- [ ] **WT-inf.2** -- **A clock that colludes with a timed wait**, so a mock can make a long wait pass
  quickly with the code waiting none the wiser (the working position in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#working-position-mock-clocks-and-timed-waits-not-a-decision)).
  Gated on the first timed wait that accounts time through a clock: its affordance depends on that
  wait's shape.
