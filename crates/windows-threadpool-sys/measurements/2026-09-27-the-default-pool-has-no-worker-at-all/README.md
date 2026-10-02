# The default pool has no worker at all, and the parked three were never its own -- 2026-09-27

This answers the question the hooks were built for, and it **corrects two
earlier findings in this investigation**.

Reading `NtQueryInformationWorkerFactory` for every worker factory in the
process, at the stalled moment and before the probe submits anything:

| | the factory with `ThreadMaximum` 768 | the factory with `ThreadMaximum` 3 |
|---|---|---|
| **stalled**, 12 captures | total **0**, waiting **0** | total 3, waiting 3 |
| **healthy**, 20 runs | total **1**, waiting 1 | total 3, waiting 3 |

Per-capture figures are in [factory-counters.csv](factory-counters.csv). Every
capture in each arm is identical; the groups above are the whole population.

## There are two worker factories in the process, and only one is ours

768 is the default process pool's thread maximum. That factory is the one this
crate's waits, timers and I/O are registered with, and it is the one whose
counts change between a passing run and a stalled one: **one worker when the
delivery is served, zero when it is not.**

The other factory has a maximum of 3, holds exactly 3 workers, has all 3
waiting, and is **byte-for-byte identical in both arms**. It does not
participate in the fault. What it belongs to is not established here; what is
established is that it is not the pool under test and that nothing about it
differs between a run that works and a run that does not.

## The correction

[the-workers-are-there](../2026-09-27-the-workers-are-there/README.md) read a
dump taken while stalled, found three threads parked in
`ZwWaitForWorkViaWorkerFactory` under `TppWorkerThread`, and concluded that the
pool was not starved of threads. Every part of that observation was accurate.
The inference was not: **those three threads belong to the other factory.**
`TppWorkerThread` is the worker routine for every pool in the process, so a
stack cannot say which factory a parked worker serves -- and a count of three
parked workers matching a factory whose maximum is three, in a process where
the pool under test reports zero, is not a coincidence.

So the finding it overturned stands after all. The default process pool has
**no worker**, in a stalled run, which is what
[the-pool-has-no-worker](../2026-09-27-the-pool-has-no-worker/README.md)
originally said from thread counts and what the dump was taken to disprove.

It also explains
[the-parked-workers-are-never-used](../2026-09-27-the-parked-workers-are-never-used/README.md)
rather than leaving it strange. No thread alive at the stall ever runs a
callback, and the backlog is served by threads created afterwards, because the
pool that owes those callbacks has nothing to run them on and has to make one.
The parked workers were never candidates.

## What the factory says about itself while stalled

Beyond the zero, the same capture reports for the 768-maximum factory:

- `Paused` **false** -- it has not been told to stop;
- `Shutdown` **false** -- it is not being torn down;
- `MayCreate` **true** -- it believes it is allowed to make a worker;
- `ThreadMinimum` 0, so nothing obliges it to keep one;
- `PendingWorkerCount` 0 and `ReleaseCount` 0 -- nobody has asked it for a
  worker;
- `LastThreadCreationStatus` 0 -- no creation has failed.

So the question is now as narrow as this investigation can make it from
outside the kernel: **a factory with no workers, permitted to create one, not
paused and not shut down, holds a queued wait-completion packet and does not
create one -- until `NtReleaseWorkerFactoryWorker` asks it to.** Of the four
ways into this pool, that call is on the work-submit path and on no other
([the-submit-is-what-releases-it](../2026-09-27-the-submit-is-what-releases-it/README.md)).

## What it does not establish

It does not show the packet in the port. The counters describe workers, not
queued completions, so "the factory has the packet and will not act on it" and
"the packet never reached the port" are still both consistent with these
numbers. The ordering evidence -- the five-second-old wait served *ahead of*
the work item that woke the worker, in 99 of 99 -- argues for the first, and
remains the argument rather than a direct reading.

It does not identify the other factory. That is a loose end, not a gap in this
result: it is identical in both arms.

## How it was run

`trace::worker_factory_counts()` was added to `windows-threadpool-sys` and
called from the reproducer's post-mortem, before the pool-liveness probe, under
a `Once` so that only the first of the two victim threads reads -- the second
would otherwise describe a pool the first had already released. The healthy arm
calls the same function at the end of a passing test.

No hooks were installed for this measurement. The factory handle is found by
asking each candidate handle whether it is a worker factory, which is
read-only; an earlier arm that installed hooks to learn the handle took 0.15 s
to 0.58 s to patch -- **after** the onset, which is bracketed to the first
15.7 ms -- and so never learned one. The failure rate with the scan in place
was 12 in 4000, inside the range this configuration produces without it.
