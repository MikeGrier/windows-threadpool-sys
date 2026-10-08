# Checklist: win-time-sys

Safe Rust over the Windows clocks. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## WT-M1 -- The crate and its clocks

- [x] **WT-1.1** -- The crate exists: a Windows-only workspace member registered for release and publication, with its README as the crate documentation and an undocumented `unsafe` refused by the build. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-11)

- [x] **WT-1.2** -- The two layers: `Timeline`, `TimePoint<T>`, `Ticks<T>` and `Clock`, with their arithmetic and conversions to and from `Duration`. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-12)

- [x] **WT-1.3** -- Interrupt time and unbiased interrupt time: two timelines and their four clocks, plain and precise. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-13)

  > **-> CROSS-COMPONENT HANDOFF:** `InterruptClock` now exists, which is what durable-ioring was
  > waiting on: component `crates/durable-ioring` -> `DI-M3` -> `DI-3.2.4.2` (nullifiers). See
  > [CHECKLIST.md](../durable-ioring/CHECKLIST.md). `WT-1.4` onwards do not block it.

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
