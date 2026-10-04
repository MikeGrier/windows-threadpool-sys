# Closing the wait too soon after disarming it is the poison -- 2026-09-28

The teardown decomposition, finished. It found two things that prevent the
stall, which makes this the first result in the investigation with a candidate
fix in it.

Per-arm figures in [arms.csv](arms.csv); the control's failure cycle indices in
[control-failures.csv](control-failures.csv).

## The decisive run

Three arms, **interleaved**, 20000 runs each, pipes drained asynchronously and
a 60-second bound per run:

| arm | teardown | failures in 20000 |
|---|---|---|
| `hand-full` | disarm -> `WaitForThreadpoolWaitCallbacks(TRUE)` -> close | **10** |
| `hand-disarm-sleep-close` | disarm -> **sleep 1 ms** -> close | **0** |
| `hand-disarm-drainwait-close` | disarm -> `WaitForThreadpoolWaitCallbacks(FALSE)` -> close | **0** |

Against the control's 10, a zero has probability `exp(-10)` = **4.5e-5**. Both
candidates are that, independently.

The arms ran round-robin, so drift affects all three equally, and it did not
arise: 157.7, 155.6 and 156.3 ms per run, a spread of 1.3%. No arm timed out.

## The whole decomposition

| teardown | runs | failures | |
|---|---|---|---|
| never armed, then closed | 4000 | 0 | |
| armed, abandoned | 4000 | 0 | |
| armed, then closed while still armed | 4000 | 0 | |
| disarm, never closed | **15000** | **0** | disarm alone is innocent |
| disarm -> close | 8000 | 11 | **poisons** |
| disarm -> cancel-drain -> close | 20000 | 10 | **poisons** |
| disarm -> sleep 1 ms -> close | 20000 | **0** | prevented |
| disarm -> true drain -> close | 20000 | **0** | prevented |
| full teardown, then sleep 1 ms | 8000 | 1 | **poisons** |

Read down the column: nothing before the disarm matters, the disarm alone does
nothing, and the poison appears exactly when a **close follows a disarm
closely**. Put any real delay between those two calls and it stops.

## The control that makes the sleep mean something

`hand-sleep-after` does the identical 1 ms sleep, placed *after* the whole
teardown rather than between the disarm and the close -- and it still fails.
So the sleep is not helping by delaying the trigger's completion, or by
shifting when the trigger finishes relative to the victims arming. **The gap
has to be in that one place.**

## Cancelling is not draining, and only draining helps

`WaitForThreadpoolWaitCallbacks` takes `fCancelPendingCallbacks`:

- **TRUE** cancels a pending callback and returns. This is what `EventDelivery`
  does, and `hand-full` shows it does **not** prevent the stall.
- **FALSE** waits for a pending callback to actually run. `hand-disarm-drainwait-close`
  shows it **does** prevent it.

The two differ by one argument and produce 10 failures against 0 over the same
20000 runs each.

The arm was written expecting it might *hang*: the trigger signals the event
while the wait is armed, so a callback is usually pending, and in a poisoned
process it would never run. It never hung -- zero timeouts in 20000, against a
60-second bound. Whatever the disarm does to a pending callback, this call
returns.

## What this is, and what it is not

It is a **workaround with a mechanism-shaped hint**: the stall needs a
`CloseThreadpoolWait` issued close behind a `SetThreadpoolWait(NULL)`, on a
wait whose event the kernel's `IoRing` also holds.

It is **not** a diagnosis. Nothing here says what the close races, or why the
worker factory is left unable to make its first worker
([the-factory-never-makes-its-first-worker](../2026-09-27-the-factory-never-makes-its-first-worker/README.md)).
A 1 ms sleep that makes a race disappear is evidence of a race, not an
explanation of one, and it would be a poor thing to ship on its own.

It also does not establish a *threshold*. One millisecond was chosen as about
80 times the natural 12 microsecond gap; nothing here says whether 10
microseconds or 100 would do, and the honest shape of that question is a
sweep, not a single number.

## On the rate, since it moved during this work

The pooled rate across every measurement of an unmodified teardown is about
2.6 failures per 1000 runs, and individual 4000-run measurements have landed
anywhere from 4 to 17 -- which is the 95% Poisson band around that mean, so
most of the apparent instability was sampling noise on small counts. A
chi-square over the six such measurements gives p = 0.048, which is evidence of
some real variation but weak.

The practical consequence governed this experiment's design: at 4000 runs a
*zero* is strong (p = 3e-5 against the pooled rate) but "11 against 5" is
noise. Every rate comparison in earlier arms should be read that way, and this
run used 20000 per arm for exactly that reason.

The control's ten failures fall at cycles 3882, 6642, 8429, 8504, 8778, 9188,
9704, 13560, 18766 and 19871 -- spread across the run rather than bunched,
which is what a constant rate looks like.
