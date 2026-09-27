# At the system clock tick, the trace window is empty -- 2026-09-27

The same standing timer as
[2026-09-27-the-pool-never-starts](../2026-09-27-the-pool-never-starts/README.md),
with its period changed from a hundred milliseconds to **15.625 ms** -- the
default Windows timer interval, 64 ticks a second, and the finest period a
process can ask for without raising the machine's global timer resolution with
`timeBeginPeriod`. A shorter request lands on the same tick, so this is the
floor for this instrument as built.

## What was observed

13 failures in 4000 runs, inside the range this configuration has produced all
day (13, 21, 14, 18, 24, 14, 10, 18). A timer expiring 64 times a second does
not prevent the fault.

Per-capture figures are in [firings.csv](firings.csv), generated from the
captures rather than typed. The summary that holds in all 13:

- the heartbeat is created and armed in the first few tens of microseconds, and
  is therefore **due at about 15.7 ms**;
- it fires **once**, at the release, having missed **320 consecutive expiries**
  in every capture -- the minimum and the maximum are both 320;
- the first pool callback of any kind in the process is the post-mortem probe's
  work item, about a tenth of a millisecond ahead of the heartbeat's one firing.

## The new thing this capture shows

At 100 ms the claim was "not one pool callback of any kind is dispatched before
the release". At the tick the instrument is fine enough to say something
plainer: **the trace itself is empty across the window.**

```text
      0.002465s t1724404 wait                   armed
      0.002465s t1724404 syscall-leave          SetThreadpoolWait
      0.002466s t1724404 delivery               armed
      0.002467s t1724404 delivery               setup-signalled
      5.014038s t1724400 postmortem             delivery-wait-expired
```

Those two lines are adjacent in the capture. `records_in_between` in
[firings.csv](firings.csv) is 0 for all 13 captures, and the trace is recording
every bracketed Win32 call as well as every callback -- so in the window there
is no callback, no syscall, and no exception. Nothing at all happens in the
process except three threads sitting in their waits.

## A tighter bracket on the onset

The heartbeat is armed **before the trigger test does anything**. In all 13
captures the heartbeat's `armed` record precedes the trigger's first
`delivery event-attached` record; the gap is roughly 0.00004 s against
0.00015 s.

So the timer was registered while the pool was not merely healthy but
*untouched by the trigger*, and it still never fired. Combined with its 15.7 ms
due time, the onset is bracketed to the interval between process start and
15.7 ms. The trigger's whole create-and-drop completes inside the first
0.2 ms of that interval.

## What it still does not say

It does not settle the queued question of whether the pool would **ever** have
dispatched in these processes. The heartbeat's first expiry at 15.7 ms is still
*after* the deliveries are armed at about 2.5 ms, so there is no scheduled
callback anywhere in the process earlier than the trigger's own work, and
therefore no successful dispatch to compare against.

It also shows that the shorter period the queued item asks for is not reachable
this way: 15.625 ms is the floor for a thread-pool timer in a process that has
not called `timeBeginPeriod`, and calling it would change the machine's timer
behaviour under the measurement.

A wait armed on an **already-signalled** event at process start does not have
that problem: it is due immediately, needs no timer resolution, and is in the
kernel-delivered class that fails. If it fires in the first fraction of a
millisecond, the pool did dispatch before the trigger ran and the fault was
induced afterwards; if it does not fire, the pool never dispatched in this
process at all. That is the re-planned next step.

## How it was run

The three reproducer tests in
[tests/event_delivery.rs](../../tests/event_delivery.rs) were temporarily given
a `arm_heartbeat()` call creating one process-wide `ThreadpoolTimer` whose
callback does nothing but `rearm_after(Duration::from_micros(15_625))`. The
compiled binary was looped 4000 times with `WINDOWS_THREADPOOL_TRACE='*'` and
every non-zero exit captured. The patch is not committed; the three captures
here and [firings.csv](firings.csv) are the artifact.
