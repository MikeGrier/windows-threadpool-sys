# The factory never makes its first worker -- 2026-09-27

Your suggestion, and it worked: install the hooks **before the tests start**.
Not by delaying the tests, but by moving the install earlier than them --
into a `.CRT$XCU` static initialiser, which the C runtime calls before `main`,
when the test harness has not yet created a thread.

That is strictly better than a delay. There are no threads to suspend, so the
patch costs nothing and perturbs nothing, and no test has to know the
instrument exists.

With the hooks in place from process start, the stall has a one-line
description.

## The result

| | first worker announces `NtWorkerFactoryWorkerReady`, relative to the delivery being armed |
|---|---|
| **healthy**, 10 runs | **0.243 -- 0.309 ms** |
| **stalled**, 8 captures | **5003 -- 5017 ms** -- that is, only at the release |

Per-capture figures are in [first-worker.csv](first-worker.csv).

A healthy run, from the capture:

```text
0.135419 t2182044 wait      armed
0.135670 t2178528 wfactory  ready-enter    a=28     <- a new worker, announcing itself
0.135673 t2178528 wfactory  park-enter     a=28
0.135675 t2178528 wfactory  park-leave     a=28     <- takes the queued packet at once
0.135676 t2178528 wait      trampoline-entered      <- the callback runs
```

The event signals, the kernel makes a worker, it announces itself, parks, is
handed the packet immediately, and runs the callback. A quarter of a
millisecond, end to end.

In a stalled run **none of that happens**. There is no `ready`, and no `park`
on that factory, at any point before the release -- in 8 of 8. The first
`NtWaitForWorkViaWorkerFactory` call against the default pool in the entire
process is the one that follows the work submit five seconds later.

So the stall is not a worker that fails to wake, or a packet handed to the
wrong thread, or a callback that runs and goes missing. **The pool's first
worker is never created.**

## What this rules out

**The `AlreadySignaled` race, which was the reason the wait registration was
hooked at all.** `NtAssociateWaitCompletionPacket` has an out-parameter the
kernel sets when the object is *already* signalled at association time; on that
path it queues no completion and leaves the caller to act, which is a second
delivery path taken only on a race and exactly the shape of a lost callback.

It is not what happens here. Across every association made before the first
callback, the flag is **false** -- 3 of 3 in each stalled capture, and in every
healthy one. The only `true` observed anywhere is a re-arm *after* the release,
which is the ordinary case: a callback re-arming a wait on an event that is
still set.

## What is left, and where it is

The factory, while stalled, reports `TotalWorkerCount` 0, `Paused` false,
`Shutdown` false, `MayCreate` **true**, `ThreadMinimum` 0 and
`LastThreadCreationStatus` 0
([the-default-pool-has-no-worker-at-all](../2026-09-27-the-default-pool-has-no-worker-at-all/README.md)).
Nothing forbids it from creating a worker, and no creation has been attempted
and failed. It owes a callback and does not make the thread to run it, until
`NtReleaseWorkerFactoryWorker` asks -- which only the work-submit path calls
([the-submit-is-what-releases-it](../2026-09-27-the-submit-is-what-releases-it/README.md)).

Every step outside the kernel is now accounted for. Why a worker factory with
no workers, permitted to create one, does not create one for a packet it has
been given is inside the kernel, and no instrument available to this workspace
reaches there.

## What it does not establish

It still does not show the packet in the port. "The kernel queued the
completion and the factory did not act on it" and "the kernel never queued it"
both produce exactly these records, because every observation here is of calls
*ntdll* makes, and neither case involves one. The association returning success
with `AlreadySignaled` false says the packet was armed, not that it was later
delivered.

## Cost, and what it changed

The failure rate with all four hooks installed was 8 in 4000, inside the range
this configuration produces without them. Installing before `main` is why: the
patch happens with no other thread running, so the suspend-and-resume pass that
makes patching safe finds nothing to do.

One measured detail worth keeping. The install pass is dominated by
`CreateToolhelp32Snapshot`, which snapshots every thread on the machine and is
then filtered to this process -- about 0.12 s. Paying it per hook put the last
of six installs 0.73 s into the process; taking one snapshot for the whole
batch puts all of them at 0.132 s. Neither is late enough to matter now that
they precede `main`, but the per-hook version would have been, had more hooks
been added.
