# Nothing ever asks the factory

2026-09-30. `M-T5.2`, first measurement. Fourteen stalls out of 8400 processes on
the `hand-spin-3us` arm. Counts in [captures.csv](captures.csv), one full trace in
[stalled.txt](stalled.txt).

**This kills the third hypothesis of the investigation**, and the data that killed
it had been sitting unread in the capture buffer for three days.

## The gap that made it possible

The layout this crate reads from `WorkerFactoryBasicInformation` has always
carried two flags -- whether the factory is queued for a deferred thread
creation, and whether a deferred-create timer is armed. They were decoded into
the struct and **never emitted**. Every capture before today was silent about
precisely the state that separates the two live readings of `M-T5.1`.

Emitting them cost one record.

## The reading

With `M-T5.1`'s result beside it, the stalled pool's complete state is:

| quantity | stalled | healthy, idle |
|---|---|---|
| completion port depth | **2** | 0 |
| total workers | 0 | 0 |
| may create | 1 | 1 |
| create in progress | 0 | 0 |
| queued for deferred create | **0** | 0 |
| deferred timer set | **0** | 0 |
| paused / shutdown | 0 / 0 | 0 / 0 |
| last creation status | 0 | 0 |
| binding count | 1 | 1 |

All fourteen captures agree exactly; there is no spread to report. The healthy
column is the same pool read early in a passing run, before it has anything to
do.

## What it means

**The stalled factory is in a perfectly clean idle state -- while holding work.**

Every field reads exactly as a factory with nothing to do: not paused, not shut
down, permitted to create, nothing in progress, nothing scheduled, nothing
failed. The only field that differs from a genuinely idle factory is the one it
does not consult unprompted: its port has two packets on it.

So the hypothesis that the deferral machinery is wedged -- that a factory stays
flagged as queued for a creation nobody services -- is **false**. It is not
flagged, and no timer is armed. Nothing is pending, which means **nothing is
scheduled to ever ask it again**.

That is a stronger and simpler statement than the wedge it replaces: the stall is
not a creation that got lost. It is a prompt that never happened.

## Three hypotheses, three refutations

Recorded because the pattern is now the most reliable thing this investigation
has produced:

1. *The pool is poisoned when a wait is closed without its queued callback having
   run.* Fitted every cell of a 2x2 and was directly observed -- refuted by a
   graded gap sweep, where an arm that leaves the callback pending in almost
   every run still reached zero.
2. *A one-at-a-time creation gate is stuck.* Refuted by a capture taken three days
   earlier that recorded the counter as 0.
3. *The factory stays flagged as queued for a deferred create.* Refuted here, by
   a field the same captures had already been reading and discarding.

In all three cases the refuting data either already existed or cost one record to
obtain. The lesson is not to hypothesise less but to **emit more of what is
already in hand before reasoning about what is not**.

## What is still open

What normally turns "a packet arrived on the completion port" into "ask the
factory to create a worker", and why it does not happen here. Everything now
points at that link rather than at the factory's own decision-making: the create
test would approve if asked, and the factory's own records show it is not going to
be asked.

The next measurement is a direct one, and it is reachable from user mode. Post a
packet to the stalled pool's completion port and see whether a worker appears.
`NtReleaseWorkerFactoryWorker` is already measured to release the stall every
time, and it reaches the factory by a different route than queued work does. If
an ordinary post also releases it, the insert-to-factory link works and something
about the victims' own inserts failed; if it does not, that link is severed, and
the difference between the two routes is the fault.

## Method

Both reads happen in the post-mortem and **before the liveness probe**, because
that probe submits work and submitting work is measured to release this stall
every time. The port depth is read without dequeuing. The port and factory are
found by asking each candidate handle what it is, since a stalled process is
precisely one where no hooked call has carried a handle.

Measured against ntoskrnl 10.0.26100.9457, ntdll 10.0.26100.9278.
