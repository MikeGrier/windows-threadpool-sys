# The `self-heal` feature

**Default: on.** It costs a timestamp store on each callback and, only once a
cancellation has actually happened, one lazily-created private thread pool with a
periodic timer.

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

1. Each callback this crate dispatches stamps a per-pool "last dispatch" marker
   before calling your closure. The source is the interrupt-time counter, a
   memory read rather than a syscall.
2. `try_cancel_pending` marks its pool as owing a repair, and stamps when.
3. A self-heal timer, running on a private pool this crate creates **lazily on
   the first cancellation**, periodically checks the marked pools. For each one
   it submits a pre-created work item -- unless a dispatch has been observed
   *since* the cancellation, which is direct evidence the pool is alive and makes
   the repair unnecessary.

The repair work item is created once, when the pool is registered, so the healing
path performs no allocation and makes one call.

## What it costs

| | cost |
|---|---|
| per callback dispatched | one interrupt-time read and one relaxed store |
| per `try_cancel_pending` | two stores |
| if you never cancel | the private pool is never created; no timer, no thread |
| after a cancellation | one private pool, one periodic timer, one work object per watched pool |
| a busy process | usually no repair submitted at all, because dispatches are observed after the cancel |

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

If you need to check, [`trace::worker_factory_snapshot`] reports every worker
factory's counts. A pool with queued work, zero workers, and permission to create
one is the signature. It is behind the `trace` feature and reads a layout
Microsoft does not publish.

## Why the ungated method is `unsafe` when hanging is not undefined behaviour

Because the keyword is carrying a transferred obligation, not a warning. The
precondition -- "ensure the pool is repaired" -- is something a caller can
discharge and a reviewer can check, which is what `unsafe` is for. Marking a
method `unsafe` merely because it can hang would dilute the keyword in a crate
that wraps a genuinely unsafe API and needs it to stay sharp.

The reasoning, and the alternatives rejected on the way to this shape, are in
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#cancellation-self-heals).
