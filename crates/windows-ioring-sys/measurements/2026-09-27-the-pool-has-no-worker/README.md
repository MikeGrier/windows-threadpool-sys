# The pool has no worker during the stall, and runs-long does not change that -- 2026-09-27

Two questions in one experiment, prompted by a reading offered during review:
that the pool is not dispatching because it has no thread, and will not grow
promptly because nothing told it these callbacks may run long.

The first half is supported and is the sharpest evidence yet about the
mechanism. The second is not: announcing the callbacks as long-running changes
nothing measurable.

## Why this reading was worth testing

This workspace has already measured what `SetThreadpoolCallbackRunsLong` does
to a pool, in
[DESIGN-NOTES.md](../../../../DESIGN-NOTES.md) -> "`SetThreadpoolCallbackRunsLong`
is the growth mechanism, not a hint": four threads created immediately, then
growth throttled to roughly one thread per 166 ms without the flag, against a
millisecond with it. A stall that ends only when the pool is given a reason to
add a thread fits that shape closely enough to test rather than argue about.

## What was measured

Two arms, four thousand runs each, and a thread count taken **in the
post-mortem** at two moments: while the stall is in progress, and again after
the work submit that ends it.

| Arm | Failures | Runs |
|---|---|---|
| `default` (control) | 21 | 4000 |
| `default-runslong` | 12 | 4000 |

Thread counts, identical in both arms and across all six captures:

| Moment | Threads in the process |
|---|---|
| while stalled | 6 |
| after the work submit | 8 or 9 |

## What follows, and what does not

- **The pool has no worker to dispatch on, and makes one when work is
  submitted.** The count rises by two or three at exactly the moment dispatch
  resumes, in every capture. This is the observation
  [CHECKLIST.md](../../CHECKLIST.md) -> `M26.13` experiment 1 was queued to get,
  and it turned out to need no external instrument -- a Toolhelp snapshot in the
  post-mortem is enough.
- **Runs-long does not fix it.** 12 failures against 21 is not a finding: the
  control arm has itself measured 13 and 21 in two separate four-thousand-run
  measurements on the same build, so 12 sits inside its own spread. No effect is
  claimed in either direction.
- **The flag really was applied.** Each arm asserts its own environment before
  use -- the runs-long arm that the flag bit is set and that it is still on the
  default pool, the private arms that a pool is named -- and records both in the
  trace as `env-built`. The assertion was checked by sabotage: removing the
  `set_runs_long` call makes the arm fail with the message the assertion
  carries.
- **It is not our callbacks that occupy the threads.** Across the twenty-seven
  captures taken for `M26.13.3`, `M26.13.4` and `M26.13.5`, **zero** trampolines
  are entered during the stall. No callback of this workspace's is inside a
  closure, blocked or otherwise, so none can be holding a worker. That is what
  the entry/exit pairing added by `M26.13.1` exists to distinguish, and it is
  the direct refutation of the narrow form of the reading.

What remains unexplained is narrower than before and can be stated in one
sentence: **why the pool will create a worker for a submitted work item but not
for a wait, timer, or I/O callback that is already queued.**

## A methodological result, recorded so it is not repeated

The first version of this experiment took its thread count on the setup path,
once per `EventDelivery` construction. A Toolhelp snapshot enumerates every
thread on the system, and three of them per run -- taken exactly where the race
happens -- cut the observed failure rate to roughly one in several thousand and
slowed each run by an order of magnitude. The run was abandoned and the count
moved into the post-mortem, which executes only after the failure has already
occurred; the control then returned to its usual rate.

This is the hazard the trace facility's own module documentation is built
around, arriving through a different door: an instrument cheap enough to leave
in a callback is not automatically cheap enough to put on a setup path.