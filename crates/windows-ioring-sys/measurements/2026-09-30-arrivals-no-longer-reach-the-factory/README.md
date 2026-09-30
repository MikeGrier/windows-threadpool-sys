# Arrivals no longer reach the factory

2026-09-30. `M-T5.6`. Thirteen stalls out of 8400 processes on the
`hand-spin-3us` arm, each poked with an ordinary completion packet from inside
its own post-mortem. Counts in [captures.csv](captures.csv); the stalled trace in
[stalled-poked.txt](stalled-poked.txt) and the control in
[healthy-poked.txt](healthy-poked.txt).

**The link from "work arrived" to "make a worker" is severed.** Work can be
delivered to the stalled pool's port and nothing will ever notice.

## The question

`M-T5.1` and `M-T5.2` between them left exactly one thing to decide. The work is
on the port, the factory would approve a create if asked, and nothing is
scheduled to ask it. So is the route from arrival to create decision **broken**,
or did it simply **never fire** for the two packets the victims happened to
insert?

Posting a packet exercises that route on demand, in a process already stalled.

## The result

| | before poke | after poke |
|---|---|---|
| **stalled**, 13 of 13 | depth 2, workers 0 | **depth 4, workers 0** |
| **healthy control** | depth 0, workers 0 | **depth 0, workers 1** |

Unanimous; there is no spread to report.

The packets **land** -- depth goes 2 to 4, so the port accepts them and the post
succeeds. No worker is created, and 250ms is generous: in a healthy run a worker
announces itself within a third of a millisecond.

The control is what makes the null result mean something. Same pool, same
starting position -- factory exists, no workers, permitted to create, port empty
-- minus the fault. There, one posted packet takes the worker count from 0 to 1
and the packet is consumed back to depth 0. So posting **is** a stimulus that
creates workers, and its failure to do so in the stalled process is a property of
that process, not of the stimulus.

## What this settles

- **The fault is not in the victims' inserts.** Any arrival fails, including one
  posted by hand five seconds later through the documented public API. Nothing
  about the two original packets was special.
- **The port is fine.** It accepts packets and its depth grows correctly.
- **The factory is fine.** It reads as perfectly idle in every field and it
  creates a worker on demand when reached by the other route.
- **What is broken is the connection between them**, and it is *persistent*, not
  a missed edge -- a fresh arrival is ignored exactly as the original ones were.

That also explains the asymmetry this investigation has carried unexplained since
the beginning: `NtReleaseWorkerFactoryWorker` recovers the stall every time
because it reaches the create decision by the **other** route, a count of
user-mode release requests, which does not depend on the severed link.

## What it does not settle

Why that link breaks. It is inside the kernel's queue-to-factory notification,
and no instrument available here reaches it. What is now established is its
*shape*: a per-port, persistent loss of notification, caused by removing a
delivered packet a few microseconds after it was queued, on a port whose factory
has no threads yet.

The `M-T5.3` question -- whether the hazard window is anchored to the queueing or
to the disarm -- is worth more now than when it was parked, because the answer
bears directly on which step of that notification is being lost.

## Method, and its one destructive step

The poke is destructive: a raw packet is not a real work item, so a worker that
appeared might dispatch it as garbage. That is acceptable only because it runs
inside a process that has already failed and is about to panic, and it is why the
arm sits behind its own environment switch rather than running by default.

The measurement is deliberately the **worker count**, not whether anything
sensible ran, for that same reason.

Order matters and is preserved: the poke happens after the depth and factory
reads and **before** the liveness probe, because that probe submits work and
submitting work is measured to repair this stall every time.

One parsing correction, recorded because it nearly cost a capture: handle numbers
vary between processes, and the first extraction hardcoded the ones seen in an
earlier run. It silently dropped the single capture whose handles differed. Both
the stalled factory and its port are now identified **by value** -- the factory
with no workers, the port with work on it -- and never by handle.

Measured against ntoskrnl 10.0.26100.9457, ntdll 10.0.26100.9278.
