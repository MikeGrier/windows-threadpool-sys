# The `self-heal` feature

**Default: on.** Callbacks this crate dispatches do not touch its state at all;
the cost begins at a `try_cancel_pending`, and the lazily-created private thread
pool and periodic timer appear only once a cancellation has actually happened.
The table under [What it costs](#what-it-costs) is the full account.

This document is for deciding whether to turn it off. The short answer is that
you should not unless you have a specific reason, and that if you do, the crate
makes you say so at the call site rather than silently losing a guarantee.

## What it protects against

`try_cancel_pending` asks Windows to discard a thread-pool callback that has not
started yet. When the callback's completion packet has **already been delivered**
to the pool's completion port, that request removes it -- and if the removal lands
a few microseconds after the packet was queued, on a pool that has no threads
yet, the port's notification to its worker factory can be lost.

A pool in that state dispatches **nothing**. Not the waits already armed, not a
freshly armed wait, not a timer, not a completed overlapped read. Its own
counters read as perfectly healthy throughout: not paused, not shut down,
permitted to create a worker, no failed creation, zero workers. Work queues up on
the port and is never looked at.

The blast radius is one pool and all of it. A second pool in the same process is
unaffected -- but the pool normally hit is the **default** one, which is shared
with every component in the process that did not create its own. Code with no
connection to whoever cancelled stops working.

Submitting a work item recovers it immediately and completely, because that
reaches the worker factory by a route the lost notification is not on. Nothing
else observed does. So a program that submits work items near its waits sees a
latency spike easy to mistake for scheduler jitter, and a program using only
waits, timers and I/O has no stimulus that will ever help it and hangs.

The measurements behind every claim above are in
[windows-ioring-sys/measurements](../windows-ioring-sys/measurements), indexed
from [STALL-TIMELINE.md](STALL-TIMELINE.md). They are
linked rather than restated so that the figures have one home.

## What this crate's normal paths do about it

**Nothing, because they do not need to.** `Drop`, `stop_and_drain`, and the
cleanup group's release all *drain* -- they let the queued callback run, which
clears the association, so the close that follows has nothing to remove. That is
structural, not probabilistic: the drain path cannot reach the removal primitive
on any code path, whatever the timing.

`self-heal` exists for the one case the drain does not cover: a caller who
explicitly asks to cancel.

## What the feature actually does

1. `try_cancel_pending` stamps its pool with when the cancellation happened. The
   source is the interrupt-time counter, a memory read rather than a syscall.
2. A self-heal timer, running on a private pool this crate creates **lazily on
   the first cancellation**, periodically checks the stamped pools. For each one
   whose cancellation has not yet been answered, it submits a pre-created work
   item to that pool.
3. That repair running is what marks the pool answered -- the submission is the
   remedy, and the callback running is the evidence it arrived.

The repair work item is created once, when the pool is registered, so the healing
path performs no allocation and makes one call. That matters because the pool it
is aimed at may already be wedged.

**A repair the pool does not take is retried once, then reported.** If a repair
is still queued five seconds later, the timer records `repair-overdue` and
submits again. Past one such reattempt the `fail-fast` feature, if enabled, ends
the process; without it the timer goes on reporting and re-submitting. The
allowance is restored whenever a repair actually runs.

## What it costs

| | cost |
|---|---|
| per callback dispatched | nothing: callbacks this crate dispatches do not touch the self-heal state |
| per `try_cancel_pending` | one clock read and one atomic max |
| if you never cancel | the private pool is never created; no timer, no thread |
| after a cancellation | one private pool, one periodic timer, one work object per watched pool |
| a pool that was cancelled | one work submission per healer period until a repair dispatches |

**Evidence of health comes only from this crate's own repair dispatching.** An
earlier design stamped a marker in every callback this crate dispatched, so
ordinary traffic on a pool would suppress the repair and a busy process usually
submitted none. That was removed: a pool with no other traffic was
indistinguishable from a wedged one, which is precisely the case the feature
exists for. The cost moved with it -- the per-callback store is gone, and a
cancelled pool now gets a repair submitted whether or not it is otherwise busy.

The coalescing matters because cancellation tends to appear in `Drop` paths as a
matter of course. Repairing on every call would charge a teardown-sized cost to a
routine operation; batching over the timer period makes it affordable enough to
be on by default.

## If you turn it off

`try_cancel_pending` **does not exist** when the feature is off, and code calling
it fails to compile. That is deliberate: a guarantee you were relying on has been
removed, and a compile error is the only way you find out.

What exists instead, always:

```text
unsafe fn try_cancel_pending_no_heal_tracking(&self)
```

Its safety obligation is not about memory. It is that **you must ensure the pool
is repaired**. Discharge it by one of:

- submitting a work item to the same pool after cancelling, which recovers it
  immediately; or
- knowing that pool is kept continuously busy by something else, so any stall is
  bounded by the next submission that would have happened anyway; or
- establishing that the pool always has at least one worker. Note this is
  weaker than it sounds -- a worker retires after its idle timeout, measured at
  67 seconds for the default pool, and the pool is exposed again from then.

## What to watch for if you disable it

The failure is silent and intermittent, and will not look like a cancellation
bug. Symptoms, in rough order of how often they would be misread:

- **Sporadic latency spikes** in a process that mixes work items with waits,
  timers or I/O. The spike ends when something submits work, so its length is
  whatever the gap to your next submission happened to be.
- **A hang** in a process that uses only waits, timers or I/O, because nothing it
  does will ever recover the pool.
- **Timeouts attributed to the wrong subsystem**, since the component that stops
  is rarely the one that cancelled.

What it will **not** look like: a crash, a leak, an error return, or anything
attributable to the call that caused it. The counters a diagnostic would normally
check all read healthy.

The signature, if you have a way to observe it, is a pool with queued work, zero
workers, and permission to create one. **This crate gives you no way to observe
it.** Reading those counters needs an undocumented entry point and an unpublished
structure layout, which this crate does not ship; an ETW kernel trace reaches the
same conclusion on documented ground, and is what the captures under
`measurements/` used to confirm it independently.

## Why the ungated method is `unsafe` when hanging is not undefined behaviour

Because the keyword is carrying a transferred obligation, not a warning. The
precondition -- "ensure the pool is repaired" -- is something a caller can
discharge and a reviewer can check, which is what `unsafe` is for. Marking a
method `unsafe` merely because it can hang would dilute the keyword in a crate
that wraps a genuinely unsafe API and needs it to stay sharp.

The reasoning, and the alternatives rejected on the way to this shape, are in
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#cancellation-self-heals).
