# windows-waitable-queues

Bounded producer/consumer queues whose readiness is a waitable Windows `HANDLE`.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

## Queues, and the moment a consumer has to wait

A bounded queue -- a ring of slots with producers at one end and a consumer at
the other -- is how one thread hands work to another without the two sharing
mutable state. The producer pushes; the consumer pops; the ring is fixed in size,
so a full queue is backpressure rather than unbounded memory growth.

The interesting moment is when the queue is **empty**. The consumer has nothing
to do and must decide how to wait for the next item. Spinning answers instantly
and burns a core doing it, so any queue meant for real work offers a blocking
receive instead, and needs something to sleep on until a producer wakes it.

**What it sleeps on is the design decision this crate is about.** Every
general-purpose Rust queue picks an internal primitive of its own -- a condition
variable, a futex, a parking lot. That is exactly right when the queue is the
only thing the thread is waiting for.

## On Windows, a thread is rarely waiting for only one thing

The realistic wait is a disjunction:

> a message arrived **or** my I/O completed **or** shutdown was signalled

Windows is built for that. A `HANDLE` is the platform's universal waitable
currency: `WaitForSingleObject`, `WaitForMultipleObjects`,
`MsgWaitForMultipleObjects`, a thread-pool wait, and alertable waits all take
one, and an I/O completion, a process exit, a timer, and a cancellation event
are all handles. A thread can wait on any mixture of them in a single call.

**A queue is the one thing in that list that is not a handle** -- because the
primitive it sleeps on is private to it.
[`crossbeam-channel`](https://docs.rs/crossbeam-channel) blocks in `recv` but
exposes no handle, and its `Select` composes only channel operations, with no
way to register a foreign OS object;
[`crossbeam-queue`](https://docs.rs/crossbeam-queue) does not block at all.
These are good queues; they simply cannot appear in the wait above.

So the thread must poll one source while blocking on another -- burning a core,
or adding latency to whichever source lost.

## What this crate contributes

**The queue's readiness *is* a `HANDLE`.** That is the whole idea, and everything
else here follows from it: a queue that hands out a handle composes with
everything the platform can already wait on, so the disjunction above becomes one
call instead of a polling loop.

Nothing is given up to get it. Every shape here can still be polled, or blocked
on directly through `recv`, without the caller ever touching a handle -- and the
kernel object is created lazily, so a consumer that only polls never allocates
one.

## The shapes

There is deliberately **no type named `Queue`**. What a queue must support --
how many threads push, whether a slot can be claimed before the message exists
-- decides its *algorithm*, not merely its configuration, so these are separate
shapes rather than one type with switches. A caller names the shape it wants.

| Shape | Producers | What it adds | Applies when |
|---|---|---|---|
| `spsc` | one | nothing -- no compare-and-swap on either side | exactly one thread pushes |
| `slotwise_mpsc` | many | Vyukov's per-slot sequence protocol, so producers push without a lock | many threads push and a full queue may refuse |
| `reserving_mpsc` | many | claiming a slot *before* the message exists | a message must not be lost to a full queue |
| `permit_mpsc` | many | an experimental claim protocol | behind `experimental-permit-claim`, outside the semver promise -- see below |

Every shape has one consumer. `permit_mpsc` is behind the non-default
`experimental-permit-claim` feature and is outside the semver promise; it will
either be merged into `reserving_mpsc` or deleted.

**Cardinality is enforced by the compiler, not by a sentence in a doc comment.**
Each shape splits into a producer handle and a consumer handle, and "single
producer" means the handle is not `Clone`. The handles are also not `Sync`, so
one that can be neither cloned nor shared is held by exactly one thread.

| Shape | Producer | Consumer | Reserves |
|---|---|---|---|
| `spsc` | not `Clone` | not `Clone` | yes |
| `slotwise_mpsc` | `Clone` | not `Clone` | **no** |
| `reserving_mpsc` | `Clone` | not `Clone` | yes |
| `permit_mpsc` | `Clone` | not `Clone` | yes |

The shapes also disagree about their smallest usable capacity, and the error
says so rather than the documentation: `spsc` accepts one slot, while the MPSC
shapes need two, because a per-slot sequence cannot distinguish "just published"
from "free again next lap" in a one-slot ring.

The capability traits over the shapes -- `Producer`, `Consumer`, `Bounded`,
`Waitable`, `Reserving` -- each shipped with the second implementation that
validated them, rather than being designed against one.

`reserving_mpsc` carries one further choice, and the next section is entirely
about it: it packs its claim position beside a reservation count in a single
word, and how those bits divide is the caller's to pick.

The set is expected to grow. Shapes with many consumers, and shapes that signal
when space becomes available so a producer can wait for room, are both under
consideration for a future revision.

The decisions all of this was built against are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

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
divided is now a caller's choice. A narrower count field buys position bits, and
what it costs is reservations held simultaneously: `Producer::reserve` takes
`&self` and returns an owned `Reservation`, so a single producer can hold as
many as the field allows, and a caller that holds many at once is choosing
against the narrower layouts rather than against a producer count.

| Layout | Reservation-count field ceiling | Pushes to recurrence | At the pre-correction planning rate |
|---|---|---|---|
| `Balanced` (default) | 4,294,967,295 | 2^32 | about 37 seconds |
| `Enduring` | 65,535 | 2^48 | about 28 days |
| `Perpetual` | 255 | 2^56 | about 20 years |
| `Wide` (needs `dwcas`) | 4,294,967,295 | 2^64 | about 5,000 years |

The last column is arithmetic, not a measurement: pushes-to-recurrence divided by
a sustained rate of about 116 million pushes per second. **That rate predates a
correction to the probe's timing window**, which had overstated throughput -- so
the true sustained rate is lower and these horizons longer. They are kept as a
floor, saying the wrap arrives sooner than it does, which is the conservative
direction for a hazard. The horizon that matters is the one on your hardware at
your rate.

The middle column is the field's ceiling rather than the count any particular
queue reaches: admission is also bounded by capacity -- `reserve` refuses once
the ring has no room beyond the reservations already outstanding -- so the
achievable count is the lesser of the two. It is reachable where capacity allows:
one producer alone fills `Perpetual`'s 255 in a loop given a ring that large,
which `one_producer_alone_can_exhaust_the_reservation_field` pins. For `Balanced`
the capacity bound binds first, since that
layout accepts at most 2^31 slots on a 64-bit target, and 2^30 on a 32-bit
one. For the others the field is the smaller number only once the queue is at least that large: a `Perpetual` queue of capacity 64 can hold 64 reservations, not 255. The achievable count is always the lesser of the two.

```rust
use windows_waitable_queues::reserving_mpsc::{self, Perpetual};

// The same queue, with a claim position that outlives the process.
let (tx, rx) = reserving_mpsc::bounded_as::<u32, Perpetual>(64)?;
# let _ = (tx, rx);
# Ok::<(), windows_waitable_queues::CapacityError>(())
```

**A deeper position is the same exchange on the same word.** `Balanced`,
`Enduring`, and `Perpetual` all issue the same exchange on the same 64-bit word
and differ only in shift and mask constants, so there is no structural reason for
one to be slower -- but **what that costs in throughput is not established**: a
probe comparing them found them indistinguishable at low producer counts, and at high counts sat outside the probe's same-code control but too close to it to establish an ordering or a cost on this host. `Wide` is a separate matter: it needs a 128-bit exchange,
and the whole push path was measured as slower under it at every producer count
measured -- smallest at one or two, several times by thirty-two, in the isolated
regime -- and it is the only thing in
this crate that costs a third-party dependency.

The default remains `Balanced` so that no existing caller's behaviour changed
when the choice was introduced. Under it, a queue driven past 2^32 pushes by two
or more producers can **silently lose an item** -- the defect described above.
`Enduring` and `Perpetual` move that point out by 2^16 and 2^24 respectively, and
`Wide` moves it to 2^64 pushes.

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

**The exposure, as arithmetic over a disclosed rate.** Under `Balanced`, 2^32
pushes is about 37 seconds of *sustained* pushing at the rate the layout table
above discloses. Two producers is the smallest count that can trigger the defect
at all. **That rate predates a correction to
the probe's timing window** and is kept as a floor for the reason the layout
table above gives: the correction lowers the rate and lengthens the horizon, so
these figures say the wrap arrives sooner than it does, which is the
conservative direction for a hazard. That is sustained throughput, not a total
accumulated over an uptime. Reaching the wrap is necessary but not sufficient: a
producer must also be stalled inside a window a few instructions wide. Rare, but
a preemption is enough, and "rare" over billions of pushes is not "never".

The figures in the table above scale that same rate model by the position
width, so they are a floor on time rather than a forecast: a queue that must
drain cannot sustain the fastest rate shown, and a slower producer takes
proportionally longer to reach its wrap.

**What bears on it.**

- **Naming a layout moves it.** `Perpetual` puts the recurrence about twenty
  years out. **What it costs in throughput is not established** -- it issues the
  same atomic compare-exchange on the same `u64` as the default, and was measured
  as indistinguishable from it at low producer counts; at high counts sat outside the probe's same-code control but too close to it to establish an ordering or a cost on this host.
- **`slotwise_mpsc` does not have this hazard** under any layout. Its positions
  are 64 bits on every target, so the equivalent wrap needs 2^64 claims. It does
  not offer `Reserving`.
- **`spsc` never had it**, having no contended claim to race.
- **The default layout's exposure is a count, not a rate.** Two conditions must
  both hold: two or more producers (one producer has no race to lose), and 4.3
  billion pushes accumulated over the life of one queue. A lower sustained rate
  does not remove the exposure -- the position advances once per push regardless
  of how fast they arrive, so a slow queue with two or more producers reaches the
  same wrap, just later. An earlier version of this bullet listed a low rate as
  its own exemption, which was wrong.

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
`portable-atomic`. `Perpetual` reaches roughly
twenty years before its claim position recurs with no dependency, though what
that costs in throughput is not established, while under `Wide` the whole push
path was measured as slower at every producer count measured -- smallest at one
or two, several times by thirty-two, in the isolated regime. What `Wide` provides
that the `u64` layouts do not is a 64-bit position: the recurrence moves to
2^64 pushes -- about 5,000 years at the same rate the table above uses, rather
than the twenty `Perpetual` buys. That is a longer horizon, not the absence of
one, and it moves with the caller's rate like every other figure in that column.

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

1. take everything available -- `pop` until it reports `Empty`, and stop
   outright if it reports `Disconnected` instead;
2. `arm()`, and if it returns `false`, start again -- something arrived;
3. **`pop` once more, and stop if it reports `Disconnected`.** `arm()` reports
   only whether a later *push* can be missed, so on a queue with no producers
   left it still returns `true` -- having just cleared the single doorbell ring
   their drop left behind. Waiting on the strength of that `true` never wakes.
   This step is not belt-and-braces either: a producer may push *and then* drop
   between step 1 and here, and that item is delivered rather than discarded
   because `pop` reports `Disconnected` only once the queue is genuinely empty;
4. only now, wait on the handle.

Step 3 used to read "check `is_disconnected()`, and if the producers are gone,
take one last time" -- two calls whose order the caller had to get right,
because a `pop` returning `None` could not say which situation it was in.
`TryRecvError` collapses that into one question with the ordering built in.

`recv` already does all four. The steps matter when driving the handle
yourself -- through a `ThreadpoolWait`, or a `WaitForMultipleObjects` across
several queues -- because then there is nothing to delegate to.

## Choosing between `slotwise_mpsc` and `reserving_mpsc`

They are **two different claim protocols**, not one queue with a switch. `slotwise_mpsc`
is Vyukov's bounded array queue: a producer asks a slot's own sequence number
whether it is free. `reserving_mpsc` counts free slots against the consumer's
position, which is the only way a reservation can be answered at all. Both are
well-studied designs in production use elsewhere, which is why this crate ships
both rather than picking one for you.

**What distinguishes them:**

- **Pushing more than ~4 billion items through one queue, from two or more producers?**
  Under its default layout `reserving_mpsc` can lose an item past that volume.
  The count is cumulative over that queue's whole life, not per run: many short
  bursts reach the wrap as surely as one long one.
  `slotwise_mpsc`'s positions are 64 bits under every configuration, and naming a
  deeper layout on `reserving_mpsc` moves the recurrence out -- `Perpetual` to
  about twenty years -- though what that costs in throughput is not established.
  The mechanism is in
  [the section on recurrence](#how-long-reserving_mpsc-runs-before-its-claim-position-recurs)
  above.
- **Of the two MPSC shapes, only `reserving_mpsc` offers `reserve`**;
  `slotwise_mpsc` structurally cannot. (`spsc` has it too, and the experimental
  `permit_mpsc` exposes its own.) Wanting `reserve` no longer means accepting the
  default layout's recurrence, but the trade is not gone -- it changes axis: a
  deeper position is paid for with a lower ceiling on outstanding reservations,
  65,535 under `Enduring` and 255 under `Perpetual` against `u32::MAX` under the
  default.
- **`spsc` requires exactly one producer and one consumer**, and does less work
  than either MPSC shape because of it.

The measurements below are one host's observation, recorded with the parameters
that produced them. They are not a ranking, and which shape suits a given
deployment is the deployment's question.

### <a id="what-was-measured"></a>What was measured

In ns per operation, isolated regime (producers only,
capacity large enough that nothing is refused). Each cell is the median of three
whole-probe runs, followed by the full range across all fifteen repetitions those
runs contain. An operation is one successful push for the three queue shapes;
for `baseline_fetch_add` it is one `fetch_add`, which is why the column is
labelled per operation rather than per push:

| producers | `slotwise_mpsc` | `reserving_mpsc` | `permit_mpsc` | `baseline_fetch_add` |
|---|---|---|---|---|
| 1 | 6.3 (6.3-7.5) | 5.4 (5.4-6.4) | 7.9 (7.9-8.3) | 2.3 (2.3-2.7) |
| 2 | 50.6 (19.3-59.5) | 31.9 (22.5-35.2) | 44.2 (37.4-45.9) | 12.1 (5.8-14.3) |
| 4 | 91.6 (89.7-99.9) | 37.2 (31.6-41.5) | 31.8 (30.4-32.9) | 13.8 (12.6-17.6) |
| 8 | 138.6 (126.9-157.6) | 37.8 (34.3-41.7) | 25.9 (25.0-27.4) | 14.7 (13.9-15.9) |
| 16 | 218.0 (188.9-272.7) | 47.9 (44.9-56.0) | 21.8 (20.9-25.6) | 14.8 (14.4-15.9) |
| 32 | 224.7 (131.4-268.3) | 51.3 (40.7-55.4) | 21.9 (20.7-39.0) | 15.0 (14.7-15.7) |

**The ranges are the point, not a footnote.** `slotwise_mpsc` at two producers
spans 19.3 to 59.5 within one configuration on one host,
and at thirty-two, 131.4 to 268.3. A median quoted without that is an anecdote,
which is why
[D-observations-not-verdicts](../windows-platform-probes/DESIGN-NOTES.md#d-observations-not-verdicts)
obliges every published figure here to carry its run count *and* its dispersion.
An earlier version of this table published the medians alone and did not meet
that obligation; the probe now carries the range through to the report so it
cannot be omitted again.

**Attribution, because a figure without it is not reusable data:**

| | |
|---|---|
| Host | `x86_64 16p/8c smt+ L2[2,2,2,2,2,2,2,2] ec[0:16] numa[16]` |
| Profile | release |
| Sampling | 50,000 pushes per producer, median of 5 repetitions, one untimed warmup pass |
| Runs | 3 whole-probe invocations; cells are the median of the three, ranges span all 15 repetitions |
| Instrument | `probe-queue-contention`, built from `fecd352` (the commit that added the range columns) |
| Taken | 2026-09-15 UTC-07:00 |

The banner's `numa[16]` is a single NUMA node holding all sixteen processors, so
nothing here says anything about cross-domain behaviour. `permit_mpsc` is behind
`experimental-permit-claim` and is not covered by the semver promise.
`baseline_fetch_add` is N threads incrementing one `AtomicU64` -- the cheapest
thing N threads can do to a contended line, included so the queue figures can be
read against what this processor does to such a line at all.

**Read these as one machine's numbers.** Producer counts above 8 oversubscribe
this host's 8 physical cores, and the spread is not small at either scale.
*Between* runs: `slotwise_mpsc` at sixteen producers gave whole-run medians of
225.7, 218.0 and 192.9. *Within* a run the probe reports its own per-row spread
-- a fourth, separate invocation of the same build gave that row a median of
226.5 over a 181.5-242.3 range across its five repetitions.
The parenthesised ranges in the table above are the wider quantity: the extremes
over all fifteen repetitions of the three captured runs. The probe's same-code
control for this regime has been measured at 0.69-1.12x over seven runs -- the
drained regime's is wider, at 0.68-1.27x, and does not apply to the isolated
figures above; see
[DESIGN-NOTES.md](../windows-platform-probes/DESIGN-NOTES.md#d-variance-is-a-finding).
That seven-run sweep is a **separate capture** taken to size the noise floor, not
a longer version of this table -- its medians differ from the ones above, which is
the point it was making. Where the two disagree, this table is the attributed
figure for this crate and the sweep is the evidence about how much such a figure
moves.

**A previous version of this table compared two hosts** -- an AMD EPYC 7763 slice
and a Snapdragon X2 Elite -- and has been removed rather than carried forward. Its
figures predate a correction to the probe's timing window, which timed from the
coordinator's clock rather than the producers' own and overstated throughput by a
margin that grew with producer count; and neither of those machines is available
here to retake them. The ARM64 data point is therefore gone rather than stale,
which is the lesser of the two problems. Restoring one is what M2.15 in the
probe crate's [CHECKLIST.md](../windows-platform-probes/CHECKLIST.md) is for.

That comparison did carry one finding worth keeping, because it was structural
rather than numeric: it was designed on the assumption that `slotwise_mpsc` would
be the cheaper shape, and measurement said otherwise on both machines.

**What moves these numbers.** Producer count, how hard the consumer drains, and
where the threads are scheduled all change the answer -- thread placement alone
moved an SPSC handoff by 5.6x on an earlier host this workspace measured. The
`placement-probe` tool in this repository runs that measurement, and
`probe-queue-contention` runs the one above.

Two things that look like reasons to choose and are not:

- **Capacity.** `reserving_mpsc` has no single ceiling: it is the layout's, one
  bit narrower than that layout's position. On a 64-bit target `Balanced`
  reaches 2^31, `Enduring` 2^47, `Perpetual` 2^55, and `Wide` 2^62 -- the last
  being the crate-wide ceiling, which is also `slotwise_mpsc`'s, so under `Wide`
  the two shapes reach the same number and there is nothing to compare. On a
  32-bit target the crate-wide ceiling is 2^30 and every layout of both shapes
  lands there, so the difference disappears again. Either way it counts slots
  allocated up front, not items ever pushed: a ring of 2^31 slots is tens of
  gigabytes before it holds anything useful.
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

  **A deliberate absence rather than an oversight, and under consideration for
  a future revision.** It is not simply the doorbell mirrored: a blocking send
  that parked on something `WaitForMultipleObjects` cannot see would reintroduce
  the very composition problem that ruled out the existing channel crates.
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
