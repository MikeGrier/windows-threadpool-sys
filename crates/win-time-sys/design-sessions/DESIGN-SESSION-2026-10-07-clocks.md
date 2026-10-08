# Design session 2026-10-07: clocks

Resulted in [WT-D-1](../DESIGN-NOTES.md#wt-d-1) through [WT-D-6](../DESIGN-NOTES.md#wt-d-6).

**How it started.** durable-ioring needed timestamps for failures and their markings, and the engineer
chose interrupt time as its one time base ([DI-D-37](../../durable-ioring/DESIGN-NOTES.md#di-d-37)):
"extremely cheap to get and we don't have any apparent requirement for higher resolution". dioring
forbids `unsafe`, and the only safe reader in the workspace was `windows-threadpool-sys`'
crate-private one, so the assistant raised where a public reader should live.

**`std` first.** The engineer asked whether the standard types were adequate. The assistant's answer:
`core::time::Duration` is an adequate representation, but nothing in `core` reads a clock, and `std`'s
two readers are not interrupt time -- `Instant` is QPC on Windows and opaque, so it cannot be recorded;
`SystemTime` is wall-clock and can jump. Common practice: `Instant` for elapsed time within a process,
`SystemTime` for anything that leaves it, and specialised crates (`quanta`, `coarsetime`) where the
cost of the read matters.

**The crate.** The engineer asked for `win-time-sys`, described in C++ `std::chrono` terms: a QPC
clock, an interrupt-time clock, and a system-time clock. On the follow-up questions:

- Interrupt-time variants: the engineer cares most about the one that does not enter the kernel and
  costs least -- the plain one -- would prefer more precision, wants the relative costs, and would
  ideally have all of them as options. The assistant's account of the costs was qualitative (reads of
  the shared page versus reads of the timer hardware) and proposed a probe rather than quoted figures.
- QPC: first a newtype over `Instant`, then reversed by the engineer -- a clock over QPC's raw ticks.
- The cycle counter: the engineer asked about `rdtscp`; the assistant noted it is a stable intrinsic
  on x64 but has no published frequency. Later dropped by the engineer for that reason.
- System time precision: under a microsecond for the precise getter, the last system tick for the
  coarse one. The engineer pointed out that both return the same format and differ only in the getter.
- `clock_cast`: not wanted -- the C++ terms were only for names.
- Time zones: none of Rust's standard types carry one; `time`, `chrono` and `jiff` do. The crate's
  points stay zone-free.

**The layering.** The engineer's model -- "the data type is the ratio and maybe what time 0 means. The
fact that there are different 'getters' is orthogonal" -- asked whether it could be layered. The
assistant mapped it to chrono's own structure: a timeline (period and epoch) as the data type, and a
clock as a getter on it, with unbiased interrupt time its own timeline because sleep separates it from
interrupt time.
