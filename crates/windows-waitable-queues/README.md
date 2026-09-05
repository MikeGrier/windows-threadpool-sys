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

`reserving_mpsc` packs its claim position beside a reservation count in one
word, and **how those bits are divided is a caller's choice**: `Balanced`,
`Enduring`, and `Perpetual` trade a reservation ceiling nobody reaches for a
claim position that lasts from about 37 seconds to about 20 years of sustained
maximum-rate pushing, at no measured cost. See
[the section on recurrence](#how-long-reserving_mpsc-runs-before-its-claim-position-recurs).

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

## How long `reserving_mpsc` runs before its claim position recurs

**`reserving_mpsc` can lose an item after 2^32 pushes under its default layout,
on every target -- not only 32-bit ones.** That layout gives the claim position
a 32-bit half of a packed word, so this reaches x86-64 and ARM64 exactly as it
reaches i686. Read that sentence before the paragraph below, because the phrase
"32-bit position" invites the opposite reading and this project has already had
to correct that misreading once.

**This is a property of the default layout, not of the shape**, and that is a
change: it was previously a defect a caller had to live with. The claim word
packs an outstanding-reservation count beside the position, and how its bits are
divided is now a caller's choice. Reservations are bounded by how many producers
are mid-send -- hundreds at most -- so giving up a ceiling nobody reaches buys
positions:

| Layout | Outstanding reservations | Pushes to recurrence | At sustained maximum rate |
|---|---|---|---|
| `Balanced` (default) | 2^32 | 2^32 | about 37 seconds |
| `Enduring` | 65,535 | 2^48 | about 28 days |
| `Perpetual` | 255 | 2^56 | about 20 years |
| `Wide` (needs `dwcas`) | 2^32 | 2^64 | unreachable |

```rust
use windows_waitable_queues::reserving_mpsc::{self, Perpetual};

// The same queue, with a claim position that outlives the process.
let (tx, rx) = reserving_mpsc::bounded_as::<u32, Perpetual>(64)?;
# let _ = (tx, rx);
# Ok::<(), windows_waitable_queues::CapacityError>(())
```

**A deeper position costs nothing measurable.** `Balanced`, `Enduring`, and
`Perpetual` all issue the same exchange on the same 64-bit word and differ only
in shift and mask constants; a probe comparing them found no difference outside
noise. `Wide` is the exception: it needs a 128-bit exchange, which measured 2-3x
slower on the claim, and it is the only thing in this crate that costs a
third-party dependency.

The default remains `Balanced` so that no existing caller's behaviour changed
when the choice was introduced. It is not the recommended layout.

**What happens.** A producer checks that there is room, is descheduled, and
resumes after other producers have driven the position field through a complete
wrap. Its claim then succeeds against a value that is numerically identical but
a whole generation later, and it writes into a slot whose emptiness was decided
long ago. If that slot now holds an item the consumer has not taken, the item is
overwritten.

**The failure is silent.** No error, no panic, no counter moves. The consumer
receives a different item than the one that was sent, and nothing observable
says so -- which is why this is documented here rather than left to a caller to
discover, and why it cannot be mitigated after the fact.

**The exposure, measured rather than estimated.** Under `Balanced`, 2^32 pushes
is 37 seconds to roughly four minutes of *sustained* pushing at this crate's own
measured rates -- about two minutes at two producers, which is the smallest
count that can trigger it at all. That is sustained throughput, not a total
accumulated over an uptime. Reaching the wrap is necessary but not sufficient: a
producer must also be stalled inside a window a few instructions wide. Rare, but
a preemption is enough, and "rare" over billions of pushes is not "never".

The figures in the table above scale that same measurement by the position
width, so they are a floor on time rather than a forecast: a queue that must
drain cannot sustain the fastest rate measured, and a slower producer takes
proportionally longer to reach its wrap.

**What to do about it.**

- **Name a layout.** `Perpetual` puts the recurrence about twenty years out at
  no measured cost, which takes it past any real deployment. This is the answer
  for almost every caller who is exposed at all.
- **`slotwise_mpsc` does not have this hazard** under any layout. Its positions
  are 64 bits on every target, so the equivalent wrap needs 2^64 claims. Prefer
  it unless you need `Reserving`.
- **`spsc` never had it**, having no contended claim to race.
- **The default layout is sound below its wrap.** A queue that will not push 4.3
  billion items in one run, or that is not driven at sustained maximum rate by
  two or more producers, is not exposed even on `Balanced`.

This is disclosed on the same principle as the ordering gap below: an adopter
gets the information we have rather than an assurance we cannot support. The
difference between the two is worth stating plainly -- an unverified ordering is
a *risk* of a bug, while this is a known one with a computed exposure. What has
changed is that the exposure is now a number the caller sets rather than one the
crate imposes.

## Cargo features

Both are off by default, and the default build depends on `windows-sys` alone.

**`dwcas`** adds the `Wide` claim layout, a 128-bit claim word for
`reserving_mpsc`. This is the only thing in the crate that costs a third-party
dependency: Rust's standard library has no 128-bit atomic -- `core::sync::atomic`
stops at 64 bits -- so the double-width compare-and-swap comes from
`portable-atomic`. Most callers do not need it; `Perpetual` reaches roughly
twenty years before its claim position recurs with no dependency and no measured
cost, while the 128-bit exchange measured 2-4x slower on the claim itself. Take
it when you want the recurrence gone as a guarantee rather than deferred by an
argument about deployment lifetimes.

**`experimental-permit-claim`** adds `permit_mpsc`, a different claim protocol in
which the decision and the operation are one atomic rather than two. It is
**not** covered by this crate's semver promise: it will either be merged into
`reserving_mpsc` or deleted once it has been measured enough to decide.

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
blocking receivers use rather than treating the handle as a readiness
predicate. That protocol has **four** steps, and the fourth is the one that is
easy to leave out:

1. take everything available;
2. `arm()`, and if it returns `false`, start again -- something arrived;
3. **check `is_disconnected()`, and if the producers are gone, take one last
   time and stop.** `arm()` reports only whether a later *push* can be missed,
   so on a queue with no producers left it still returns `true` -- having just
   cleared the single doorbell ring their drop left behind. Waiting on the
   strength of that `true` never wakes. The last take is not belt-and-braces
   either: a producer may push *and then* drop between step 1 and this check;
4. only now, wait on the handle.

`recv` already does all four. The steps matter when driving the handle
yourself -- through a `ThreadpoolWait`, or a `WaitForMultipleObjects` across
several queues -- because then there is nothing to delegate to.

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

- **Pushing more than ~4 billion items in one run, from two or more producers?**
  Use `slotwise_mpsc`. `reserving_mpsc` has a known item-loss defect past that
  volume, on every target -- see [A known defect in
  `reserving_mpsc`](#a-known-defect-in-reserving_mpsc-disclosed-rather-than-fixed)
  above, which you should read before choosing.
- Need `reserve`? Only `reserving_mpsc` has it, and `slotwise_mpsc` structurally
  cannot. Weigh that against the defect above rather than treating the
  capability as settling the choice.
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

- **Capacity.** On a 64-bit target `slotwise_mpsc` reaches 2^62 slots and
  `reserving_mpsc` 2^31. On a 32-bit one the crate-wide ceiling is 2^30 and
  **both** shapes land there -- `reserving_mpsc`'s packed 2^31 is clamped down
  to it as well -- so the difference disappears and the comparison means
  nothing. Either way it counts slots allocated up front, not items ever pushed:
  a ring of 2^31 slots is tens of gigabytes before it holds anything useful.
- **`slotwise_mpsc` winning at one producer.** True in one regime, and at one producer
  you want `spsc` anyway.

## What it will not do

- **It will not overwrite.** A full queue fails, or a reservation guarantees a
  slot. Overwrite-oldest is right for telemetry, where a lost entry is a lost
  sample; here an entry may be an I/O submission, where a lost entry is a lost
  operation.
- **It will not let a producer wait for room.** The doorbell is one-directional:
  a consumer can park until there is something to take, and there is no
  equivalent for a producer waiting until there is somewhere to put. `push`
  refuses immediately with `PushError::Full`, and `reserve` returns `None`;
  neither blocks, and no handle is offered to wait on.

  **Said plainly because the obvious comparison misleads.** `crossbeam-channel`'s
  `send` blocks on a full bounded channel, so a reader arriving from it will
  expect the same here and get a refusal instead. A producer with nowhere to go
  must decide what to do -- shed the item, retry on its own schedule, or grow a
  buffer of its own -- rather than being parked by the queue.

  This is a deliberate absence rather than an oversight, and it is not
  permanent: whether a producer can wait, and **what it would wait on**, is the
  open question in [M32.3](../../CHECKLIST-io-domains.md). The constraint that
  makes it non-trivial is the one this crate exists for -- a blocking send that
  parks on something `WaitForMultipleObjects` cannot see would reintroduce
  exactly the composition problem that ruled out the existing channel crates.
  Two further wrinkles, recorded so the shape of the problem is visible: a
  bounded queue can offer this and an unbounded one never can, so it belongs in
  its own capability trait rather than in `Waitable`; and while every shape here
  has one consumer, two have many *producers*, so a room signal has N waiters
  and is not the mirror image of the doorbell.
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
