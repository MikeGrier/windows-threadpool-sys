# What a process does after the stall

2026-09-30. Answers the severity question: is a process that hits this fault
dead, permanently degraded, or merely delayed?

**Merely delayed -- but only if something submits a work item.** The evidence was
already in 63 captures taken for other reasons; no new run was needed.

## The question

Earlier work established that a stalled pool holds queued work, has no threads,
is willing to make one, and is never asked. What that leaves open is what a
*program* experiences. Three possibilities, with very different consequences:

1. the process is finished -- the pool never works again;
2. the process is permanently degraded -- it recovers but breaks again whenever
   the pool goes idle;
3. the process is delayed -- something repairs it and normal service resumes.

## What continued use actually does

Measured in [which-poke-releases-the-stall](../2026-09-27-which-poke-releases-the-stall/README.md),
fifteen captures, each giving the poke a two-second observation window:

| continued use of... | recovers? |
|---|---|
| a fresh wait, armed and signalled | **no** |
| a fresh timer, due in 1ms | **no** |
| a real overlapped read that completed | **no** |
| nothing at all | **no** |
| **a work item submitted** | **yes, within microseconds** |

So "keep using the thread pool and nothing bad shows up" is **false** for waits,
timers and I/O: those are exactly what stops. The one thing that recovers it is a
work submit, which reaches the factory by the route that does not depend on the
broken notification.

## And recovery is complete

The post-mortem probe in every stall capture does two things in order: it submits
a work item, and then it arms a **brand-new** wait, signals it, and waits up to
two seconds. Its verdict appears in **63 captures across six experiments**, and
it is the same in all of them:

```
pool liveness  : work item ran: true; a fresh wait ran: true (both within 2s)
```

[recovery.txt](recovery.txt) shows the sequence: the work submit at `5.014174s`,
the five-second-old backlog draining at `5.015601s`, a fresh wait signalled at
`5.015729s`, and answered at `5.015730s` -- microseconds later.

So possibility 1 is out, and within the observed window possibility 2 is out too:
a wait armed *after* recovery dispatches normally.

## What this means for a program

The fault's cost is **everything stops until the next work submit**, then
everything resumes -- including the backlog, served ahead of the work item that
woke the pool.

Two consequences worth stating separately, because they point opposite ways:

- **A program that submits work items anywhere near its waits, timers or I/O
  experiences a latency spike, not a hang.** It is bounded by the interval to the
  next submit. That is easy to mistake for scheduler jitter and hard to attribute,
  which makes the fault far more likely to be shrugged off than diagnosed.
- **A program that uses only waits, timers and I/O has nothing to recover it.**
  Nothing in that set is a stimulus. It hangs, and it stays hung. This workspace's
  reproducer is exactly that program, which is why the fault presented as a hang
  rather than as jitter.

## What is still open

Whether the pool breaks again once it returns to zero workers. The idle timeout is
measured at 67s, so every capture here observes a pool that still has the worker
the recovery created. A test that waits past the idle timeout and then arms a wait
would settle it; none has been run.

The distinction matters: if it re-breaks, a long-lived process is in a permanent
stop-start state rather than having had one bad moment.

## A failed experiment, recorded because the trap is cheap to fall into

The first attempt at the related question -- whether the lost notification
outlives the queue emptying -- tried to drain the stalled pool's completion port
with `GetQueuedCompletionStatus` at a **zero timeout** and then post a fresh
packet, making a genuine empty-to-non-empty transition.

**It hangs.** Six worker processes sat for fifteen minutes with no progress and
captured nothing; the call does not return despite the zero timeout. The
instrument was removed rather than kept with a warning.

Anyone reaching for the same idea should know that a plain
`GetQueuedCompletionStatus` is not a safe way to inspect a thread pool's own
completion port, whatever the timeout says. The depth can be read without
disturbing anything using `NtQueryIoCompletion`, which is what the instruments
here use.
