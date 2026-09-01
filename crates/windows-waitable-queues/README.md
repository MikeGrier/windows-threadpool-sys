# windows-waitable-queues

Bounded producer/consumer queues whose readiness is a waitable Windows `HANDLE`.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

**Status: three shapes, all waitable.** `spsc` is a bounded ring with no
compare-and-swap on either side; `slotwise_mpsc` is a bounded array queue using Vyukov's
sequence protocol, so any number of producers may push without a lock; and
`reserving_mpsc` is that queue plus the ability to claim a slot in advance. Any
of them can be polled with no kernel object at all, blocked on directly, or
waited on alongside other handles. The capability traits over them --
`Producer`, `Consumer`, `Bounded`, `Waitable`, `Reserving` -- each ship with the
second implementation that validated them.

The decisions all of this was built against are in
[DESIGN-NOTES.md](DESIGN-NOTES.md), and the remaining work is tracked in
[CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) at the workspace root.

## Why

Rust has good concurrent queues. What none of them offers on Windows is the one
property this crate is named for: **you cannot wait on them alongside a kernel
object.**

`crossbeam-channel` blocks in `recv`, but parks on its own internal primitive and
exposes no `HANDLE`; its `Select` is built purely from channel operations, with no
way to register a foreign OS object. `crossbeam-queue` does not block at all.

So a thread that needs to wake on

> a message arrived **or** my I/O completed **or** shutdown was signalled

cannot express that wait. It has to poll one source while blocking on another,
which either burns a core or adds latency.

On Windows a `HANDLE` is the universal waitable currency -- `WaitForSingleObject`,
`WaitForMultipleObjects`, `MsgWaitForMultipleObjects`, a thread-pool wait, and
alertable waits all take one. A queue whose readiness *is* a `HANDLE` composes
with everything the platform can wait on. One that hides its readiness behind a
private primitive composes with nothing.

## Plural, and no `Queue` type

This is a family of shapes, not one queue: they differ in producer and consumer
cardinality, in how they store items, and in what they do when full. None is
canonical, so there is deliberately no type named `Queue` -- a consumer names the
shape it wants.

Each shape splits into a **producer handle** and a **consumer handle**, and
cardinality is carried by whether those handles are `Clone`:

| Shape | Producer | Consumer | Reserves | Shipped |
|---|---|---|---|---|
| `spsc` | not `Clone` | not `Clone` | yes | yes |
| `slotwise_mpsc` | `Clone` | not `Clone` | **no** | yes |
| `reserving_mpsc` | `Clone` | not `Clone` | yes | yes |
| MPMC | `Clone` | `Clone` | -- | not yet |

So "single producer" is a fact the compiler enforces, not a sentence in a doc
comment: the handles are also not `Sync`, so a handle that cannot be cloned and
cannot be shared is held by exactly one thread.

The two shapes also disagree about their smallest usable capacity, and the error
says so rather than the documentation: `spsc` accepts one slot, and `slotwise_mpsc` needs
two, because its per-slot sequence cannot distinguish "just published" from "free
again next lap" in a one-slot ring.

## How far the memory orderings are verified, and how far they are not

Stated plainly, because a lock-free queue that is vague about this is asking to
be trusted rather than evaluated.

**What is verified.** Every ordering was reasoned about when written, and the
reasoning is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) beside the code it
justifies. The shapes are covered by an extensive unit suite and by a sabotage
suite that injects deliberate defects and requires each to be caught -- which is
how the one real ordering bug this crate has had was found: a lost wakeup where
the doorbell cleared its mirror flag before resetting the event.

**What is not.** Stress testing cannot catch a *weakened memory ordering* here,
and that is measured rather than assumed: changing the producer's `Acquire` load
of the consumer's position to `Relaxed` left the entire suite green, while every
logic defect injected beside it was caught. A test can only observe the
interleavings the hardware and scheduler happen to produce, and neither x86-64
nor ARM64 obliged.

**So the orderings are not machine-checked.** Verification with a model checker
is planned before 1.0. Until then `0.x` is meant literally, and an adopter for
whom that matters has the same information we do rather than an assurance we
cannot support.

One limit worth knowing even after that work lands: a model checker covers the
queue shapes' positions and sequence numbers, and **cannot** cover the doorbell,
whose correctness is the interleaving of an atomic flag with real `SetEvent` and
`ResetEvent` calls. Modelling those would verify a model of them rather than the
calls themselves.

## Where these algorithms come from

**None of the queue algorithms here are novel, and that is deliberate.** A
concurrent queue is a bad place to be original: the failure mode is a reordering
that shows up on one machine, under load, months later. Each shape implements a
published design, and what this crate adds is the waiting, not the queueing.

- **`spsc`** is the classic single-producer single-consumer ring buffer, with the
  two positions on separate cache lines so the ends stop invalidating each
  other. The structure is old -- Lamport gave the concurrent reader/writer
  treatment in 1983 -- and the padding is standard modern practice.
- **`slotwise_mpsc`** implements Dmitry Vyukov's bounded MPMC array queue,
  specialised to one consumer. Each slot carries its own sequence number, so a
  producer claims a position and asks *that slot* whether it is ready, which
  keeps producers off any single shared line. It is among the most widely
  reimplemented concurrent queues in existence.
- **`reserving_mpsc`** uses the other classic approach: count free slots against
  the consumer's position, so space can be **claimed in advance**. Credit- and
  ticket-based admission is long established in flow control, and counting is
  the only way to answer "will there be room later?".

Where this crate departs from a reference implementation it says so, and why, in
[DESIGN-NOTES.md](DESIGN-NOTES.md). The measured behaviour of both MPSC shapes is
below -- including one case where the published intuition turned out to be wrong
on our hardware.

## Why not an existing queue crate

Rust has excellent channel crates, and for most programs one of them is the right
answer. **They are not usable here for one structural reason: on Windows,
waiting is a kernel-object operation, and a queue whose readiness is not a
`HANDLE` cannot take part in one.**

A thread that must wait for *an item arrived* **or** *an I/O completed* **or**
*this process exited* **or** *cancellation was requested* waits on all of them at
once, in a single `WaitForMultipleObjects`. Every participant has to be a kernel
object. A channel that signals readiness through a condition variable, a futex,
or a parked-thread list cannot be one of them -- however good its blocking
receive is, and however rich its `select`, because that select can only cover its
own channels.

The alternatives are all worse in the same way:

- **Poll on a timer.** Trades latency against wakeups, and the thread wakes to
  discover nothing happened.
- **Dedicate a thread to blocking on the channel and signalling an event.**
  Correct, and costs a thread plus a hop per item to convert a condition variable
  back into the kernel object you needed in the first place.
- **Move everything to async.** A real answer if the program is already async;
  not one for a thread whose other obligations are `HANDLE`s.

So the queue owns a manual-reset event and keeps it **never unsignalled while
there is something to take**. That one-sided guarantee is the hard part and is
what this crate is actually for.

It is one-sided deliberately. The event stays signalled after the last item is
taken until the consumer clears it with `arm()`, and a producer's signal may
land after the consumer has already drained -- so a wake means *there may be
something*, never *there is something*. What the crate guarantees is the
direction that matters: a wake is never missing. Follow the protocol the
blocking receivers use -- pop, `arm()`, re-check -- rather than treating the
handle as a readiness predicate.

The event is created lazily, so a consumer that only polls never allocates a
kernel object at all.

## Choosing between `slotwise_mpsc` and `reserving_mpsc`

They are **two different claim protocols**, not one queue with a switch. `slotwise_mpsc`
is Vyukov's bounded array queue: a producer asks a slot's own sequence number
whether it is free. `reserving_mpsc` counts free slots against the consumer's
position, which is the only way a reservation can be answered at all. Both are
well-studied designs in production use elsewhere, which is why this crate ships
both rather than picking one for you.

**Start here:**

- Need `reserve`? Only `reserving_mpsc` has it, and `slotwise_mpsc` structurally cannot.
- Otherwise, **start with `reserving_mpsc`.** It was the faster of the two at
  every producer count we measured above one.
- Only one producer *and* one consumer? Use `spsc`, which beats both.

**What we measured**, in ns per push, isolated regime, median of three runs.
Higher producer counts oversubscribe both hosts:

| producers | `slotwise_mpsc` (x64) | `reserving` (x64) | `slotwise_mpsc` (ARM64) | `reserving` (ARM64) |
|---|---|---|---|---|
| 1 | 9.0 | 8.6 | 6.5 | 6.1 |
| 2 | 49.0 | 28.0 | 29.8 | 9.4 |
| 4 | 84.4 | 33.3 | 60.6 | 12.9 |
| 8 | 140.8 | 38.5 | 167.4 | 29.8 |
| 16 | 193.5 | 52.2 | 194.9 | 30.6 |
| 32 | 239.7 | 56.9 | 195.0 | 30.6 |

x64 is an AMD EPYC 7763 slice (8 cores, 16 threads); ARM64 is a Snapdragon X2
Elite (12 cores, no SMT). **Read these as two data points, not as a law.** This
comparison has already inverted once: it was designed on the assumption that
`slotwise_mpsc` would be the cheaper shape, and measurement said otherwise on both
machines.

**Measure your own workload before treating any of this as settled.** Producer
count, how hard the consumer drains, and where the threads are scheduled all
move the answer -- thread placement alone moved an SPSC handoff by 5.6x on one
of these hosts. The `probe-core-affinity` tool in this repository exists so you
can run that measurement on your hardware instead of inheriting ours.

Two things that look like reasons to choose and are not:

- **Capacity.** `slotwise_mpsc` reaches 2^62 slots and `reserving_mpsc` 2^31, but that
  counts slots allocated up front, not items ever pushed. A ring of 2^31 slots
  is tens of gigabytes before it holds anything useful.
- **`slotwise_mpsc` winning at one producer.** True in one regime, and at one producer
  you want `spsc` anyway.

## What it will not do

- **It will not overwrite.** A full queue fails, or a reservation guarantees a
  slot. Overwrite-oldest is right for telemetry, where a lost entry is a lost
  sample; here an entry may be an I/O submission, where a lost entry is a lost
  operation.
- **It will not decide between two real queue designs on your behalf.** `slotwise_mpsc`
  and `reserving_mpsc` are different claim protocols, both well studied and both
  used in production and in research. `slotwise_mpsc` asks each slot's own sequence
  number "are you free?"; `reserving_mpsc` counts free slots against the
  consumer's position, which is what makes a reservation answerable at all --
  and why `slotwise_mpsc` does not implement the `Reserving` trait. It genuinely cannot,
  which is the whole reason the traits are narrow.
  **Which is faster is a property of your workload, not of the designs**, and we
  publish what we measured rather than choosing for you -- see "Choosing between
  them" below.
- **It will not allocate on push.** Bounded shapes allocate once, at
  construction.
- **It will not create a kernel object you never use.** The doorbell is created
  lazily, so a consumer that only polls allocates none.
- **It will not destroy your items on a thread you did not choose.** A queue
  built with a `Disposal` sink hands whatever nobody drained back to you at
  teardown, rather than running the destructors inside the last handle's drop.
  That matters when an item owns a handle, because closing one can block --
  and the thread that happens to release last may be a pool callback that must
  not. Without a sink the items are destroyed in place, which is the right
  default for items that own nothing.
- **It will not round your capacity.** A capacity that a shape cannot represent
  is refused, with the nearest valid neighbours on the error, rather than
  silently turned into one the caller did not choose.

## What it will tell you about itself

Three numbers, through the `Observable` trait on either handle:

- **`refused()`** -- pushes turned away for want of room. This is the loss count,
  and it counts room only: a push refused because the consumer is gone is the end
  of the stream, not backpressure.
- **`doorbell_rings()`** -- `SetEvent` calls, not signal attempts. The difference
  between the two *is* the skip optimisation, which is what makes this the number
  worth reporting.
- **`high_water()`** -- the deepest the queue got, or `None` if nobody asked for
  it to be tracked. It is the one metric that cannot be made free, so it is
  opt-in via `Options::tracking_high_water`; `None` rather than `0` so you cannot
  mistake "nobody was counting" for "it never filled".

Depth is not on that list because `len()` already reports it, from positions the
queue keeps anyway.

## Licence

Copyright (c) Mike Grier.
