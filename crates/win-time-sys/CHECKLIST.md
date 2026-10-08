# Checklist: win-time-sys

Safe Rust over the Windows clocks. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## WT-M1 -- The crate and its clocks

- [ ] **WT-1.1** -- **Scaffold the crate.** `crates/win-time-sys`, a workspace member registered for
  release and publication, Windows-only, with a README saying what it is. Its `unsafe` is confined to
  the Win32 calls, each with its safety argument; everything it exports is safe.

- [ ] **WT-1.2** -- **The two layers** ([WT-D-2](DESIGN-NOTES.md#wt-d-2)): `Timeline`, `TimePoint<T>`,
  `Ticks<T>` and `Clock`, with ordering, subtraction to `Ticks`, adding `Ticks` to a point, and
  `Ticks` to `Duration`. Tested over a fake timeline and clock, including overflow at the edges and
  that points on two timelines cannot be compared (a `compile_fail` doctest).

- [ ] **WT-1.3** -- **Interrupt time and unbiased interrupt time** ([WT-D-3](DESIGN-NOTES.md#wt-d-3)):
  two timelines and four clocks over `QueryInterruptTime`, `QueryInterruptTimePrecise`,
  `QueryUnbiasedInterruptTime` and `QueryUnbiasedInterruptTimePrecise`.

- [ ] **WT-1.4** -- **System time**: the `FileTime` timeline and its two clocks, over
  `GetSystemTimeAsFileTime` and `GetSystemTimePreciseAsFileTime`.

- [ ] **WT-1.5** -- **Performance time**: the timeline over QPC's frequency, read once and kept, and
  its clock over `QueryPerformanceCounter`'s raw ticks ([WT-D-4](DESIGN-NOTES.md#wt-d-4)).

- [ ] **WT-1.6** -- **A cost probe**: time each clock on the machine it runs on and report what was
  observed, so the choice between getters rests on the consumer's own hardware rather than on figures
  quoted from elsewhere. Where it lives -- an example here, or `windows-platform-probes` -- is decided
  with the item.

## WT-M2 -- Adoption

- [ ] **WT-2.1** -- **`windows-threadpool-sys` reads interrupt time through this crate**, retiring its
  crate-private `heal::now()`, so the workspace has one reader of its one time base.

- [ ] **WT-2.2** -- **durable-ioring's timestamps use `InterruptClock`**
  ([DI-D-37](../durable-ioring/DESIGN-NOTES.md#di-d-37)).

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M3` ->
  > `DI-3.2.4.2` (nullifiers, whose markings and `Failed` entries are timestamped). See
  > [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

## M-inf -- Parked

- [ ] **WT-inf.1** -- **A cycle-counter clock** (`rdtscp` on x64; the architecture's virtual counter on
  ARM64, which needs `asm!`). Deferred by the engineer ([WT-D-6](DESIGN-NOTES.md#wt-d-6)), blocked on
  having no mapping from cycles to real-world time.
