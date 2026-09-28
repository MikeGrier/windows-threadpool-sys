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

## What actually runs in one process, and in what order

Two questions the check was asked to settle plainly.

**Is it several test cases in one process, or one test repeated?** Several
*different* test cases, always, and never a repeat. `libtest` runs each
`#[test] fn` exactly once per process, and this file has no parameterisation --
no `test_case`, no `rstest`, no `proptest`. The 3-test reproducer is three
distinct functions on three concurrent threads:

| thread | test | role | body |
|---|---|---|---|
| first | `dropping_with_nothing_outstanding_does_not_hang` | trigger | 6 lines |
| second | `completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting` | victim | 83 lines |
| third | `completions_queued_before_handover_are_still_delivered` | victim | 99 lines |

No run in this investigation has ever had **fewer than two** test cases in a
process: the fault needs the trigger co-running, so a single-test process
cannot produce it. All the repetition is *across* processes -- roughly thirty
thousand fresh ones -- and none within one.

**Do they overlap?** Less than the word "concurrent" suggests. The trigger
creates its `EventDelivery`, arms a wait, drops it and closes it within about
0.05 ms, and the victims arm their deliveries about 2.4 ms later. Figures in
[setup-ordering.csv](setup-ordering.csv):

| | trigger finished before the first victim armed | gap |
|---|---|---|
| failing, 80 captures | **80 of 80** | 1.837 -- 9.856 ms, mean 2.442 |
| passing, 183 captures | **183 of 183** | 1.897 -- 28.838 ms, mean 2.602 |

So the trigger is not running alongside the victims' setup at all. Whatever it
does to the pool, it has finished doing before they arrive.

**And the ordering carries no information**, which is why the passing row is
here. Failures show it 80 out of 80 times, which on its own looks like a
signature; passing runs show exactly the same thing 183 out of 183. It is
simply how `libtest` schedules these three tests, and a reading of the failures
alone would have made a discriminator out of a constant.

Seventeen further passing runs were captured and are not in that table: the
dump is emitted at the end of one victim, and in those it finished before the
*other* victim had created its delivery, so the capture holds two deliveries
rather than three. That is a limit of where the dump sits, not a second
behaviour -- and it does show the two victims do not always overlap each other.