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
Each has every clock Windows offers for it.

## What is here

- **Interrupt time and unbiased interrupt time**, each with two clocks. The plain one --
  `InterruptClock`, `UnbiasedInterruptClock` -- reads the value the kernel publishes into every
  process, so it never enters the kernel, and is as fresh as the last system clock tick. The precise
  one -- `PreciseInterruptClock`, `PreciseUnbiasedInterruptClock` -- reads the timer hardware, and is
  finer. A point means nothing after a restart: both count from the start of the boot.
- **System time**, the `FileTime` timeline: UTC wall-clock time, which means something after a
  restart and on another machine, but is not steady -- it moves when the time is set, backwards
  included. `CoarseSystemClock` reads it as of the last system clock tick; `PreciseSystemClock` to
  under a microsecond, through the same call `std::time::SystemTime::now` makes.

```rust
use std::time::Duration;
use win_time_sys::{Clock, InterruptClock, PreciseInterruptClock, TimePoint};

let start = PreciseInterruptClock.now();
std::thread::sleep(Duration::from_millis(20));
let elapsed = (PreciseInterruptClock.now() - start).to_duration().expect("forwards");
assert!(elapsed >= Duration::from_millis(19));

// The plain and precise clocks read one timeline, so their readings compare.
assert!(InterruptClock.now() <= PreciseInterruptClock.now());

// A point is recorded as its tick count, and read back.
let recorded: u64 = start.ticks();
assert_eq!(TimePoint::from_ticks(recorded), start);
```

The performance counter is planned in this crate's `CHECKLIST.md`.

## What is not here

- **Time zones.** System time is UTC and the others have no calendar meaning. Zones belong to the
  date-time crates -- `time`, `chrono`, `jiff` -- as Windows itself keeps `FILETIME` in UTC.
- **Conversions between clocks** beyond what Rust's own time types provide.
- **The processor's cycle counter.** Windows publishes no frequency for it, so a cycle count cannot
  become a duration without an estimate. It is parked rather than rejected.

The reasoning behind each is in this crate's `DESIGN-NOTES.md`.
