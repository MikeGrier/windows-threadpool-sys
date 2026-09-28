# The parked workers are never used -- 2026-09-27

Prompted by a question about the previous finding: was the thread that releases
the stall the one that had been waiting?

It was not, and the two calls involved are different in kind. But the question
pointed at one that had not been asked: **the three workers the dump found
parked -- are they the ones that eventually serve the backlog?**

They are not. In a stalled run, no thread that was alive at the moment of the
stall ever runs a callback.

## The two calls, since they are easy to conflate

- `NtWaitForWorkViaWorkerFactory` is the **park**. `TppWorkerThread` blocks in
  it waiting to be handed a callback. That is where the dump found three
  threads, and the stacks are in
  [the-workers-are-there](../2026-09-27-the-workers-are-there/stalled-process-stacks.txt).
- `NtReleaseWorkerFactoryWorker` is the **post**. It never blocks. It is called
  by whichever thread runs `SubmitThreadpoolWork`, which here is the test's own
  post-mortem thread -- running, not waiting.

So the releaser and the waiters are different threads doing opposite things.

## The measurement

`snapshot_threads()` was added to the post-mortem, emitting one
`thread-present` record per live thread with its `GetCurrentThreadId` value.
The trace stamps every record with the same id, so the thread that later serves
the stalled wait can be tested for membership in that set.

A healthy control asks the same question of a passing run: the same snapshot,
taken after the delivery is armed and before any completion can arrive.

Per-capture figures are in [serving-threads.csv](serving-threads.csv).

| | threads alive at the snapshot | first delivery served by a thread that already existed |
|---|---|---|
| **stalled** (12 captures) | 6 | **0 of 12** |
| **healthy** (30 runs) | 6 | **30 of 30** |

Both populations hold six threads at the moment of the snapshot: the main
thread, the two test threads, and three others. In a passing run one of those
three serves the delivery, every time. In a stalled run none of them ever runs
anything; the backlog is served by one or two threads that did not exist when
the stall was observed.

Summed over the arms, the number of distinct serving threads that pre-existed
the snapshot is 30 of 30 in the healthy arm and **0 of 30** in the stalled arm.

## What this changes

[M26.13.11](../2026-09-27-the-workers-are-there/README.md) read the dump as
"the pool is not starved of threads". The literal claim survives -- the threads
are there, parked, in both populations. The inference drawn from it does not:
their presence was taken to mean supply is not the subject, and these runs show
the parked workers are **present and unused**. In the stalled process the pool
does not dispatch to a worker it already has; it dispatches only once a new
thread exists.

So the stall is not "a queued packet waiting for a free worker". There was a
free worker for the whole five seconds, in the same state as the one that
serves the delivery in a passing run.

## What it does not establish

It does not say whether the new thread was created *because* the parked ones
were unusable, or merely as a side effect of the submit. `TppWorkPost` calls
`TppAdjustRunningThreadGoalWithLock` on its way to the release
([the-submit-is-what-releases-it](../2026-09-27-the-submit-is-what-releases-it/README.md)),
so a submit raises the thread goal whether or not a worker is idle, and this
measurement cannot separate the two.

What it does settle is the part that needs no such separation: the **wait**
callback -- which involves no submit, and had been queued for five seconds --
is served by a thread that did not exist at the stall, in 12 of 12, while in a
passing run it is served by one that did, in 30 of 30.

It also does not identify the three parked threads as belonging to this pool by
any direct evidence. The composition is identical in both populations and one of
them serves the delivery in the healthy arm, which is the argument; it is not a
reading of the factory's own membership.

## How it was run

`snapshot_threads()` (Toolhelp `TH32CS_SNAPTHREAD`, filtered to this process)
was added temporarily to
[tests/event_delivery.rs](../../tests/event_delivery.rs), called from the
post-mortem and -- under `IORING_HEALTHY_SNAPSHOT` -- from the healthy path,
with the trace dumped at the end of a passing run. The stalled arm is 4000 runs
producing 12 failures; the healthy arm is 30 runs of the same test.
[M26.13.6](../2026-09-27-the-pool-has-no-worker/README.md) had already measured
that a Toolhelp snapshot on the setup path destroys the race while one taken
after the failure does not, which is why the stalled arm's snapshot is in the
post-mortem. The patch is not committed; the two sample captures and the CSV
are the artifact.
