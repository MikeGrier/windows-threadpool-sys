# Re-verifying the reproducer's premises -- 2026-09-28

A check of the setup itself, run from a clean build of the committed tree after
a long chain of findings had accumulated on top of it. Four arms of 4000 runs
plus a repeat, all with `WINDOWS_THREADPOOL_TRACE` set so the rates compare
with every earlier measurement. Figures in [arms.csv](arms.csv).

## What the reproducer is

Each run is **one fresh process**. The ETW capture in
[the-kernel-agrees-no-thread-is-made](../2026-09-27-the-kernel-agrees-no-thread-is-made/README.md)
recorded 900 distinct process ids for 900 runs, so this is measured rather than
assumed.

Inside that process the tests run **concurrently**: the machine has 16 logical
processors, `libtest` defaults to that many test threads, and the kernel trace
shows the three test threads created within 105 us of each other.

Outside the process there is **no deliberate load**. The loop is serial -- one
process at a time, the next starting after the previous exits. The only other
activity is whatever the machine is doing anyway. Worth stating plainly,
because "concurrent" here means *between the tests inside one process*, not
between processes.

## The arms

| arm | what runs | failures in 4000 |
|---|---|---|
| A | the 3-test reproducer: 2 victims + the trigger | **16** |
| B | the whole binary, all 7 tests | **18** |
| C | the 2 victims, **no trigger** | **0** |
| D | **one** victim + the trigger | **8** |
| A again | as A, after clearing the temp directory | **13** |

**B: the reduction is representative.** The reproducer selects 3 of the
binary's 7 tests. Running all 7 gives the same rate, so narrowing to three
changed nothing about the phenomenon.

**C: the trigger is necessary.** Zero in 4000 without it, against 16 with it.
This re-confirms from scratch what `M26.9` originally narrowed to -- the fault
needs a co-running test that creates an `EventDelivery` over an empty ring and
drops it promptly.

**D: one victim is enough, at about half the rate.** A single victim alongside
the trigger still fails. That it is roughly half of arm A fits one model better
than another: if the pool simply died once per process at some fixed rate, A
and D would be equal, because any victim present would report it. Halving
instead suggests the race is between the trigger's teardown and *a victim's*
delivery setup, so two victims give two chances to enter the state. That is the
reading the numbers fit, not a measurement of the mechanism.

## A correction the check produced

The record said the two victims "fail together, never singly". That is wrong.
Across the 80 captures committed under [measurements/](../), 79 report both and
[one reports a single victim](../2026-09-27-the-pool-never-starts/stall-3888.txt)
while the other test passes.

It is not a second phenomenon. That capture's own trace shows the pool dead for
the full five seconds -- the first callback of any kind is at 5.003027s. What
differs is the finish: the two victims' deadlines are a fraction of a
millisecond apart, the first to expire runs the post-mortem probe, the probe's
work submit releases the pool about 0.3 ms later, and the second victim's
deliveries land at 5.003056s, inside its own deadline. Whether the second
victim reports depends on whether its remaining margin exceeds the release
latency.

So both are always *stalled* together; they usually both *report*. One more way
the diagnostic probe repairs the fault it is measuring, which is the recurring
hazard of this whole investigation.

## A defect the check produced

The tests **leak their temp files**. `temp_file` builds a path under the system
temp directory and nothing ever deletes it; each run of the reproducer leaves
two behind. After this investigation's roughly thirty thousand runs the
directory held **307,383 files, 1.2 GB**, removed before the final arm.

It is not a confound: arm A repeated against a cleared directory gives 13 in
4000 against 16 with the files present, and both sit inside the range this
configuration has produced all along.

It is not confined to this file either -- 15 of `windows-ioring-sys`' test
files build paths under the temp directory and none clean up. Queued rather
than fixed here, for a stated reason: adding teardown to the reproducer changes
the test under an active investigation, and every failure rate on record would
have to be re-established against the new shape. That is a decision to take
deliberately, not in passing.
