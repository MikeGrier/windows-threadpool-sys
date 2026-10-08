# win-time-sys

Memory-safe Rust over the Windows clocks. Thin over Win32 and adding no policy -- the `-sys` suffix
is that promise, as the workspace's naming decision defines it.

## How it is organised

Two layers, kept apart because they vary independently:

- **A timeline** is what a value means: how long one tick is, and what zero is. A time point is a
  tick count on one timeline. Two points on the same timeline can be compared and subtracted, and
  can be written down and read back; points on different timelines cannot be compared at all,
  because their types differ.
- **A clock** is a way to read "now" on one timeline. Several clocks can share a timeline, differing
  only in what a reading costs and how fresh it is.

The timelines are system time (UTC, 100 ns ticks since 1601), interrupt time (100 ns ticks since
boot), unbiased interrupt time (the same, leaving out time asleep), and the performance counter.
Each has every clock Windows offers for it. They are planned in this crate's `CHECKLIST.md`.

## What is not here

- **Time zones.** System time is UTC and the others have no calendar meaning. Zones belong to the
  date-time crates -- `time`, `chrono`, `jiff` -- as Windows itself keeps `FILETIME` in UTC.
- **Conversions between clocks** beyond what Rust's own time types provide.
- **The processor's cycle counter.** Windows publishes no frequency for it, so a cycle count cannot
  become a duration without an estimate. It is parked rather than rejected.

The reasoning behind each is in this crate's `DESIGN-NOTES.md`.
