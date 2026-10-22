# Checklist: win-time-sys

Safe Rust over the Windows clocks. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.
WT-M1, the crate and its clocks, and WT-M2, its adoption by `windows-threadpool-sys` and
durable-ioring, are complete and archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## WT-M3 -- The engineer's review of the shapes chosen while building

The assistant chose these while implementing `WT-M1`, and the crate and its consumers bind to them
already; what is open is the review. A rejected point is reworked in its item, with its decision
row amended.

- [x] **WT-3.1** -- The engineer confirmed [WT-D-7](DESIGN-NOTES.md#wt-d-7) in full, with the addition that no `checked_` method ever panics. -> [completed 2026-10-22](COMPLETED-CHECKLIST.md#wt-31)

- [ ] **WT-3.2** -- **The engineer confirms or amends [WT-D-8](DESIGN-NOTES.md#wt-d-8)**, from
  `WT-1.6`: the cost probe shipped as this crate's `clock_costs` example rather than in
  `windows-platform-probes`, and reporting what it observed -- least, median and most cost per
  read, and the smallest step -- with no ranking, no verdict, and no figure quoted in the crate's
  documentation.
