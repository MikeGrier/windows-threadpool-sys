# Checklist: win-time-sys

Safe Rust over the Windows clocks. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.
WT-M1, the crate and its clocks, and WT-M2, its adoption by `windows-threadpool-sys` and
durable-ioring, are complete and archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## WT-M3 -- The engineer's review of the shapes chosen while building

The assistant chose these while implementing `WT-M1`, and the crate and its consumers bind to them
already; what is open is the review. A rejected point is reworked in its item, with its decision
row amended.

- [ ] **WT-3.1** -- **The engineer confirms or amends [WT-D-7](DESIGN-NOTES.md#wt-d-7)**, from
  `WT-1.2`: `Clock::now` taking `&self`; a point a `u64` tick count and a distance an `i64` one;
  `ticks_per_second` returning `NonZeroU64`; conversions to and from `Duration` truncating towards
  zero; and panicking operators beside `checked_` methods.

- [ ] **WT-3.2** -- **The engineer confirms or amends [WT-D-8](DESIGN-NOTES.md#wt-d-8)**, from
  `WT-1.6`: the cost probe shipped as this crate's `clock_costs` example rather than in
  `windows-platform-probes`, and reporting what it observed -- least, median and most cost per
  read, and the smallest step -- with no ranking, no verdict, and no figure quoted in the crate's
  documentation.

## M-inf -- Parked

- [ ] **WT-inf.1** -- **A cycle-counter clock** (`rdtscp` on x64; the architecture's virtual counter on
  ARM64, which needs `asm!`). Deferred by the engineer ([WT-D-6](DESIGN-NOTES.md#wt-d-6)), blocked on
  having no mapping from cycles to real-world time.

- [ ] **WT-inf.2** -- **A clock that colludes with a timed wait**, so a mock can make a long wait pass
  quickly with the code waiting none the wiser (the working position in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#working-position-mock-clocks-and-timed-waits-not-a-decision)).
  Gated on the first timed wait that accounts time through a clock: its affordance depends on that
  wait's shape.
