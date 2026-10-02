# A timer armed while the pool was healthy also stops -- 2026-09-27

Suggested in review: arm a one-shot timer for four seconds whose callback does
nothing but re-arm itself for another four, and see what it does to the failure
rate.

It does not measurably change the rate. What it does is produce the strongest
statement this investigation has: **a timer registered before anything went
wrong, due a full second before the test's deadline, does not fire until the
work submit releases the pool.**

## Why this differs from the earlier timer poke

[2026-09-27-which-poke-releases-the-stall](../2026-09-27-which-poke-releases-the-stall/README.md)
created a timer *during* the stall and it did not fire. That leaves an
objection open: perhaps a pool in this state cannot accept new registrations,
and the timer was never properly armed.

This one is armed at process start, while the pool is demonstrably healthy, and
holds a standing commitment. There is no registration to fail.

A passing run exits in about fifty milliseconds, long before the four-second
expiry, so the heartbeat only ever gets a chance to fire on a run that stalls.
That is what makes it a clean probe rather than added load.

## What was observed

10 failures in 4000 runs. In **all ten**, the timer was armed at 0.000s, was
therefore due at 4.000s, and fired only when the stall was released:

| Capture | due | fired | late by |
|---|---|---|---|
| stall-0064 | 4.000s | 5.011s | 1011 ms |
| stall-0106 | 4.000s | 5.009s | 1009 ms |
| stall-1887 | 4.000s | 5.004s | 1004 ms |
| stall-2094 | 4.000s | 5.008s | 1008 ms |
| stall-2457 | 4.000s | 5.008s | 1008 ms |
| stall-2813 | 4.000s | 5.019s | 1019 ms |
| stall-2873 | 4.000s | 5.015s | 1015 ms |
| stall-3013 | 4.000s | 5.005s | 1005 ms |
| stall-3291 | 4.000s | 5.018s | 1018 ms |
| stall-3477 | 4.000s | 5.009s | 1009 ms |

Ten of ten, about a second late every time, and never at four seconds.

**It is its own positive control.** The timer does fire, so the machinery works
and the four-second arming took effect; it simply cannot fire while the pool is
in this state. A "never fires" result would have been ambiguous. This is not.

**And it is dispatched with everything else**, on the single thread that wakes.
From [stall-0064.txt](stall-0064.txt):

```text
      5.010607s work                   submitted
      5.010882s work                   trampoline-entered
      5.010930s wait                   trampoline-entered
      5.010935s delivery               callback-entered
      5.010970s wait                   trampoline-entered
      5.010986s timer                  trampoline-entered
      5.010987s timer                  rearm-requested            4000
```

The work items, both stalled deliveries, and the overdue timer all run within
400 microseconds of one another, on one thread, after the submit.

## What it adds

Until now the strongest statement was that things *created during* the stall do
not dispatch. This is stronger, and it closes the last route by which the fault
could have been about registration rather than dispatch:

- the timer was registered while the pool was healthy;
- it involves no event, no handle, no ring, and no wait -- a pure kernel timer
  expiry;
- it was due inside the stall window;
- it did not fire;
- it fired the instant a work item was submitted.

## On the rate

10 in 4000, against 13, 21, 14, 18, 24 and 14 in 4000 for the same
configuration earlier the same day. It sits just below that spread, and one
measurement cannot separate a real reduction from ordinary variation, so **no
effect on the rate is claimed** in either direction.