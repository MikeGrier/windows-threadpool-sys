# Design notes: windows-waitable-queues (Tier 1)

This file records the decisions this crate's code is built against. D-1 to D-9 were taken during the
2026-08-30 design session and transcribed here so they steer the work rather than sitting in a session
record nothing is obliged to read; D-10 onwards were taken while building the shapes those decisions
called for, and record what the building settled or corrected. The work itself is tracked in
[CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) at the workspace root, because it spans several
components.

The naming decision -- plural, and no `-sys` suffix -- lives in the workspace
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#the-waitable-queues-crate-is-named-plural-and-carries-no-sys-suffix)
rather than here, since it was taken before this directory existed.

## Intent

Bounded producer/consumer queues whose readiness is a waitable Windows `HANDLE`.

The queues themselves are ordinary. What the crate exists for is the `HANDLE`: it lets a consumer park
on a queue **and** a kernel object in one wait, which is exactly what the existing Rust concurrent
queues cannot offer on Windows, and it is why depending on one of them was rejected rather than
preferred.

## Decisions

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **The crate exists because readiness must be a `HANDLE`, not because Rust lacks queues.** `crossbeam-channel` parks on a private primitive and its `Select` accepts only channel operations; `crossbeam-queue` never blocks. Neither can be seen by `WaitForMultipleObjects`, and neither can see an `IoRing` completion event. A consumer needing "a message **or** an I/O completion **or** shutdown" must otherwise poll one source while blocking on another. |
| <a id="d-2"></a>D-2 | **Capabilities are sliced into narrow traits, not gathered into one.** The `std::io` shape -- `Read`, `Write`, `Seek`, `BufRead` -- rather than a single fat `WaitableQueue`. Forced by the shapes themselves: a poll-only queue cannot implement a trait containing `doorbell()`, and an unbounded one cannot implement `capacity()` meaningfully. |
| <a id="d-3"></a>D-3 | **No trait ships until a second implementation exists to validate it.** The trait *shape* is fixed now so signatures stay compatible; the traits themselves land with the second shape. |
| <a id="d-4"></a>D-4 | **Every shape is split into producer and consumer handles, and cardinality is carried by `Clone`.** Single-producer becomes a compile-time guarantee rather than a documented precondition. |
| <a id="d-5"></a>D-5 | **The doorbell is level state owned by the queue: signalled exactly when the consumer has something to observe.** The **reset** must not be separable from the observation that there is nothing to take; the **signal** may be. Manual-reset, and created lazily. Realized without a lock by [D-9](#d-9). |
| <a id="d-9"></a>D-9 | **Without a lock, the reset is made inseparable from the observation by two things: ordering (clear, then re-check, and never wait if the re-check finds anything) and a `SeqCst` fence on each side.** `Consumer::arm` is the ordering step; the fences defeat the store-buffer hazard that ordering alone leaves open. The natural order -- check, then clear -- is asserted to hang by deliberate sabotage; the fences are beyond any test's reach and are M31.6's target. **Amended: this decision originally claimed the ordering alone sufficed.** |
| <a id="d-6"></a>D-6 | **Overflow fails or reserves, and never overwrites.** For telemetry an overwritten entry is a lost sample; for an I/O submission it is a lost operation, and the two must not share a policy knob. |
| <a id="d-7"></a>D-7 | **Shapes are plain modules, not Cargo features, until compile time justifies otherwise.** Two features are four configurations to test, against a benefit dead-code elimination already provides. |
| <a id="d-8"></a>D-8 | **Published, and the obligation is accepted deliberately.** Unlike `windows-guard-alloc`, this is general-purpose and its first consumer is not its only plausible one. |
| <a id="d-10"></a>D-10 | **The multi-producer shape is Vyukov's bounded array queue: a sequence number per slot, claimed by a compare-and-swap on the tail and published by a release store.** The sequence is what lets the consumer tell a *claimed* slot from a *written* one, which a plain fetch-and-add cannot. Lock-free rather than wait-free, bounded by construction, and no allocation after the constructor. |
| <a id="d-11"></a>D-11 | **The capability traits shipped with this second shape, and the signatures `spsc` wrote down in advance held unchanged.** That is [D-3](#d-3)'s check actually being run rather than assumed. The load-bearing choice was `push(&self)`: `&mut self` would have been sound for one producer and would have made the trait unimplementable by this one. |
| <a id="d-12"></a>D-12 | **A shape's *minimum* capacity belongs to the shape, not to the crate, and `mpsc`'s is two.** One slot cannot encode three states when the lap stride is the capacity, so "published at `p`" and "free again at `p + capacity`" collide. Reported through `CapacityError` rather than worked around, because every available workaround puts a load back on the producer's hot path for every queue in order to serve a capacity of one. |
| <a id="d-13"></a>D-13 | **The arming protocol is written once, in `blocking.rs`, and a shape binds to it by implementing a crate-private `Parked` trait.** The blocking receive loop *is* [D-9](#d-9), not glue around it; a second shape spelling it out again would be a second copy of a rule -- the exact mistake this crate has already paid for once. |
| <a id="d-14"></a>D-14 | **`mpsc`'s arming asks "would `pop` find something", not "is `len` zero".** The two disagree over a slot a producer has claimed but not published, and only the first answer lets the consumer park on it instead of spinning until that producer is rescheduled. |

## D-2: capabilities are sliced, not gathered

The first sketch of this crate had one `WaitableQueue` trait carrying push, pop, the doorbell, capacity,
and the loss latch. The engineer's observation that the shapes would be "sliced and diced by various
traits as we go along" is the correct instinct, and following it exposes that the fat trait is not merely
inelegant -- **it is unimplementable by the shapes that are planned.** A queue that is never waited on
has no doorbell to return; an unbounded queue has no capacity to report; a queue with no loss latch has
no losses to describe.

So the contract is a set of narrow traits, each naming one capability, and a shape implements the subset
it genuinely has. The anticipated set, which is expected to grow:

| Trait | Names | Held by |
|---|---|---|
| `Producer` | `push`, and the error a full or disconnected queue returns | producer handle |
| `Consumer` | `pop`, and drain-to-empty | consumer handle |
| `Waitable` | the readiness `HANDLE` | consumer handle |
| `Bounded` | `capacity`, `remaining` | either |
| `Reserving` | a slot claimed in advance for a message that must not be lost | producer handle |
| `LossReporting` | the coalesced loss latch | consumer handle |
| `Observable` | depth, high-water, doorbells actually rung | either |

Two consequences worth stating, because they are what the slicing buys:

- **A consumer can be generic over exactly what it needs.** The I/O domain runtime needs `Consumer` and
  `Waitable` and nothing else; making it generic over a trait that also mentions reservation and loss
  reporting would couple it to capabilities it never uses.
- **`Waitable` is not queue-specific and may not stay here.** "Hands out a `HANDLE` you can wait on" is a
  property an event, a timer, or a completion port has too. If a second kind of thing wants to implement
  it, the trait moves to a lower crate and this one depends on it. Recorded so that move is a planned
  step rather than a surprise.

## D-3: the traits ship with the second implementation, not the first

Writing a trait against one implementation designs in a vacuum: every signature the single type happens
to have looks like a requirement, and nothing tests whether the abstraction is the right one. The
workspace already prefers duplicate-then-decide for exactly this reason -- keep the speculative path
separate until it is proven, then merge or delete.

So the **shape** of the traits is fixed now, because it constrains the concrete types (D-4), while the
traits themselves are written when the second shape exists to be checked against them. The cheap
discipline that makes this work: write the intended signatures as a comment before writing the first
type, and confirm the type satisfies them.

The failure this avoids is specific and unrecoverable-in-place. If the first shape ships
`pop(&mut self) -> Option<T>` and the second ships `try_pop(&self) -> Result<T, Empty>`, no trait unifies
them afterwards without a breaking change to one.

## D-4: split handles, and cardinality carried by `Clone`

The conventions for these shapes differ, and the difference is structural rather than cosmetic. An SPSC
queue is conventionally a split `Producer`/`Consumer` pair; a shared MPMC queue is conventionally one
`Arc<Q>` with `&self` on both ends. **No trait spans those two structures**, so a crate wanting a common
contract must choose one, and split handles are the choice that generalizes.

It buys more than uniformity:

| Shape | Producer | Consumer |
|---|---|---|
| SPSC | not `Clone` | not `Clone` |
| MPSC | `Clone` | not `Clone` |
| MPMC | `Clone` | `Clone` |

Cardinality stops being a precondition in prose and becomes a fact the compiler enforces: a producer that
cannot be cloned cannot become a second producer. The alternative -- a shared `&self` queue documenting
"only one consumer" -- is precisely the rule-you-must-remember that
[`RingScope`](../windows-ioring-sys/DESIGN-NOTES.md#d-43) and `get(&mut self)` were introduced to
eliminate elsewhere in this workspace.

The consumer handle also owns the doorbell, because the consumer is what waits; a producer merely rings.

## D-5: the doorbell invariant, and which half must be under a lock

The invariant is one sentence: **the event is signalled exactly when the consumer has something to
observe.** It is *level* state -- a function of the queue's contents -- rather than a record of edges,
which is why it is manual-reset.

The asymmetry is the part that is easy to get wrong, and it was worked out by walking the interleavings:

- **The signal may be given outside any lock.** A late `SetEvent` can at worst arrive after the consumer
  already drained that item and parked, which produces a spurious wakeup: the consumer wakes, finds
  nothing, parks again. Harmless, and consumers must tolerate it regardless.
- **The reset must not be separable from the observation that there is nothing to take.** Otherwise:
  consumer drains to empty, producer pushes and signals, consumer resets -- clearing the signal for an
  item that is still there -- and parks. That wakeup is lost and the item is stranded.
  An earlier wording of this said the two must be *atomic*, which is how a lock achieves it but not the
  only way, and taken literally it would have condemned the lock-free implementation this crate actually
  ships. What is required is that no push can fall between them unnoticed; [D-9](#d-9) gets that from
  ordering plus a re-check instead of from mutual exclusion.

So a redundant signal is free and a stale reset is fatal, which is the whole reason the queue owns its
doorbell rather than accepting one. The same invariant, reached independently, is stated in
[windows-file-watcher's queue](../windows-file-watcher/src/queue.rs): signalling under the lock a
receiver holds while deciding there is nothing to take, "so a wakeup cannot be lost in the gap between
those two decisions, because there is no gap".

**What is reused from that queue is the invariant, not the implementation.** It uses `Mutex` and
`Condvar`, which is right for change-notification cadence and wrong here, because a producer-side lock
serializes exactly what multi-producer exists to parallelize.

**Created lazily**, so a consumer that only ever polls allocates no kernel object at all. Handed out as
a borrowed handle plus an owned duplicate, so a caller can choose whether to own it.

**Skipping a redundant signal is an optimization, not a requirement.** Measured on ARM64: one
`SetEvent`/`ResetEvent` cycle is ~165 ns against a ~7 ns uncontended atomic, but one doorbell per drained
*batch* costs 20.6 ns per operation at a batch of eight and 5.2 at thirty-two -- so by a batch of about
twenty-three the doorbell already costs less per operation than the push it accompanies. The cheap skip
(the queue was already non-empty) needs no knowledge of the consumer and can be taken immediately; the
one requiring the consumer to publish whether it is parked is deferred until a measurement against real
work justifies its lost-wakeup risk.

## D-9: the arming protocol, which is how a lock-free queue keeps D-5

[D-5](#d-5) says the reset must not be separable from the observation that there is nothing to take. A
lock-based queue gets that by doing both under the lock it already holds, which is what
[windows-file-watcher's queue](../windows-file-watcher/src/queue.rs) does. This crate's shapes are
lock-free by construction -- a producer-side lock serializes exactly what multi-producer exists to
parallelize -- so the property has to come from somewhere else.

It comes from **ordering plus a re-check**, and the order is the reverse of the one that reads
naturally:

1. Take everything available.
2. Clear the doorbell.
3. **Check emptiness again.** If anything is there, do not wait.
4. Wait.

`Consumer::arm` is steps 2 and 3, and returns whether step 4 is safe. Step 3 is what carries the
guarantee: an item arriving before the clear is found by the check, and an item arriving after the clear
signals a doorbell that is no longer about to be reset. There is no third case.

**Check-then-clear is the lost wakeup**, and it is the easier code to write: a push landing between the
check and the clear both signals and has its signal erased, so the consumer sleeps on a queue that is
not empty and will never be signalled again. Not a stall -- a permanent hang.

**Lazy creation is a third case of the same hazard.** A producer running while no event exists skips
signalling, because there is nothing to signal. So the doorbell must be created *before* the emptiness
check that decides to wait, which is why `arm` creates it rather than assuming a caller did.

**The ordering above is necessary and, on its own, was not sufficient -- this decision originally said
it was.** A code review found the hole. Program order does not relate the producer's decision to skip
signalling to the consumer's emptiness check, because each side *stores* one location and then *loads*
another: the producer stores the queue position and loads the doorbell state, while the consumer stores
the doorbell state and loads the queue position. That is the store-buffer shape from Dekker's
algorithm, and release/acquire permits both loads to return stale values. When both do, the item is
queued, no signal was raised, and the consumer parks forever.

The remedy is a `SeqCst` fence on each side -- before the loads in `Doorbell::signal`, after the stores
in `Doorbell::clear`. Every published eventcount carries the same fence in the same place for the same
reason, which is the clearest sign that this is a known shape rather than a local quirk.

The original text had actually *identified* the sequential-consistency requirement and then dismissed
it, on the reasoning that the re-check closed the hole for free and the fence was only needed for a
different design (signalling at creation time). That reasoning was wrong, and the shape of the error is
worth keeping: the re-check closes the *program-order* version of the hazard, which is the one that is
easy to picture, and leaves the *visibility* version, which is not. Reasoning about interleavings in
terms of "what happens first" silently assumes the sequential consistency that is exactly what is
missing.

Two temptations recorded as refused, both instances of
[PLATFORM INTEGRITY](../../.github/copilot-instructions.md) rule 2. The consumer's `ResetEvent` is a
syscall and is very probably a full barrier; and `stlr`/`ldar` on aarch64 are ordered more strongly
than the abstract model demands. Either would likely mask this defect on today's toolchain and today's
processors. Neither is a specified guarantee, and binding correctness to the incidental behaviour of a
code generator plus a particular processor -- rather than to the ordering primitives -- is the precise
trap that rule exists to name.

**No test can catch this, and that is a property of the hazard.** Removing either fence leaves the
whole suite green, and no entry in `sabotage.json` can express it, because the defect is a fact about
the memory model rather than an interleaving a scheduler can be coaxed into producing. It is the named
target of the `loom` work in [CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) item M31.6.

**This is asserted by sabotage, not by argument.** The suite reverses steps 2 and 3 deliberately and
requires the result to hang -- a real `WaitForSingleObject` that returns `WAIT_TIMEOUT` while an item
sits in the queue. The race is driven deterministically from one thread, because an interleaving that
must be hit to prove a point is not one to leave to the scheduler. Three further sabotages (push not
signalling, producer `Drop` not signalling, `clear` not resetting the mirror flag) are likewise caught
*as hangs*, which is the correct shape for this class of defect and the reason the sabotage harness
judges by exit code with a timeout rather than by reading output.

**The signal side is cheapened, and a control proves that is all it is.** An `AtomicBool` mirrors the
event so a redundant `SetEvent` costs ~7 ns instead of ~81. Removing that optimization must leave the
suite green -- and does. Had it failed, the tests would have been asserting the implementation instead
of the contract.

## D-6: overflow fails or reserves, and never overwrites

Three policies, and the absence of a fourth:

- **Fail fast.** A full queue returns the item to the caller in a typed error. That failure *is* the
  backpressure, and it is why the shapes are bounded: an unbounded queue has no backpressure to offer,
  only deferred memory growth.
- **Reserve.** A slot claimed in advance, so a message that must not be lost has somewhere to go. Taken
  from [windows-file-watcher's queue](../windows-file-watcher/src/queue.rs), which needs it for exactly
  the same reason: some messages are the ones a consumer cannot afford to miss.
- **Coalesced loss latch.** When a queue may lose, a drop latches a report the consumer is guaranteed to
  observe, so loss is *counted* rather than silent. Also from the watcher.

**Overwrite-oldest is deliberately not offered.** `crossbeam`'s `force_push` makes an `ArrayQueue` usable
as a ring buffer, which is right for telemetry, where an overwritten entry is a lost sample. Here an
entry is an I/O submission, and overwriting one is a lost *operation*. The two cases must not share a
knob, because a knob invites a consumer to choose the wrong one.

## D-7: shapes are modules, not Cargo features

An earlier position in the session was "one crate, feature-gated shapes". The first half stands -- one
crate, not a crate per family, so the shared vocabulary lives in one place. The second half does not
survive contact with the cost: two features are four configurations, and this workspace already runs a
`feature-matrix` CI job that would have to grow to cover them. The benefit -- not compiling a shape you
do not use -- is one dead-code elimination already provides for an unused type.

Feature-gating remains available if compile time ever justifies it. It is not the default, and the burden
of proof is on adding a feature rather than on leaving one out.

## D-8: published, and what that commits us to

Publishing is an obligation rather than a status. It means the API is a contract that cannot be changed
casually, that a breaking change costs a major version, and that the crate must be documented for readers
who have never seen this workspace.

It is accepted because this crate is general-purpose in a way `windows-guard-alloc` is not. That one is
`publish = false` precisely because its design trades memory for determinism and would be wrong for
anything but a test binary. These queues carry no such trap, the first consumer is not the only plausible
one, and a Windows Rust program that wants to wait on a queue and a kernel object together currently has
to write this itself.

## D-10: the MPSC shape is Vyukov's bounded array queue

The obvious multi-producer array queue claims an index with a fetch-and-add, writes the slot, and lets the
consumer read it. It does not work, and the reason is worth stating because it is the whole justification
for the extra machinery: **the consumer cannot tell a slot that has been claimed from one that has been
written.** A producer preempted between the two leaves a hole, and a consumer reading through the hole
reads uninitialized memory.

A sequence number per slot carries both facts at once. Slot `i` starts at `i`; a producer may claim
position `pos` only when the slot reads `pos`, and publishes by storing `pos + 1`; the consumer takes the
slot only when it reads exactly `pos + 1`, and frees it by storing `pos + capacity`, the position the next
lap will claim it at. A claimed-but-unwritten slot is therefore invisible to the consumer, and there is no
hole to read through.

What this buys, and what it costs:

- **Bounded by construction, so backpressure is free.** A full queue is a slot whose sequence has not come
  round, which costs one load to discover. There is no separate count to maintain, no allocation to fail,
  and no policy knob -- the refusal *is* the backpressure, which is [D-6](#d-6) in its cheapest form.
- **No allocation after the constructor**, which is what makes it usable on an I/O submission path.
- **Lock-free, not wait-free.** A producer that loses its compare-and-swap retries, with no bound on how
  many times it may lose. What is guaranteed is that some producer always makes progress, and -- the
  property that actually matters here -- that a producer suspended by the scheduler blocks no other
  producer. It blocks only the consumer's view of the items queued behind it, and only until it resumes.
- **Order is claim order, not publication order.** If producer A claims position 5 and producer B claims
  6 and publishes first, the consumer must wait for A. This is not a defect to engineer around: it is what
  makes the queue a FIFO at all. B's signal wakes a parked consumer that then finds nothing, which is a
  spurious wakeup the protocol already tolerates, and A's own signal follows when it publishes.

The head and the tail are padded onto separate cache lines. The padding is load-bearing and looks like
waste, which is why it is commented at both fields rather than at one: every successful push writes the
tail and every successful pop writes the head, so adjacent they would false-share, and each write would
invalidate the other side's copy of a value it only reads. That cost has no symptom other than being
slow, which is exactly the kind that survives a code review.

## D-11: the traits shipped here, and the check D-3 demanded was actually run

[D-3](#d-3) said no trait ships until a second implementation exists to validate it, and that the trait
*shape* would be fixed in advance so the concrete types could not diverge. `spsc` accordingly wrote its
intended signatures into its module documentation before its types existed. This milestone is where that
promissory note came due.

**The signatures held unchanged.** `push`, `pop`, `is_disconnected`, `capacity`, `len`, `is_empty` are
what [`traits.rs`](src/traits.rs) says now and what that comment said then. The check is not rhetorical:
`mpsc` is a lock-free array queue with a per-slot state machine and no structural resemblance to a
two-position ring, so a signature fitted to the first shape would have failed here rather than in a
consumer's code.

**One choice turned out to be the load-bearing one, and it is worth naming.** `push(&self)` rather than
`push(&mut self)`. `&mut self` would have been perfectly sound for a single producer, is what several SPSC
crates use, and would have made this trait *unimplementable* by a shape whose whole point is several
threads pushing at once. It was chosen in advance on the argument that one spelling has to serve every
shape; this shape is the evidence that the argument was right.

Two smaller decisions recorded so they are not re-litigated:

- **The traits are also the names of the concrete handles.** `Producer` and `Consumer` are both a trait
  and, in each shape's module, a type. That is deliberate -- the trait is named for the role, the handle
  is named for the role, and the handle plays the role -- and `std` does the same with `fmt::Write` and
  `io::Write`. A caller wanting only the methods imports them anonymously (`Consumer as _`).
- **`Reserving`, `LossReporting` and `Observable` from [D-2](#d-2)'s table are deliberately still absent.**
  They belong to work that has not happened (M31.2, M31.4), and shipping an empty trait now would be the
  design-in-a-vacuum D-3 forbids, one level up.

## D-12: the minimum capacity belongs to the shape, and mpsc's is two

`spsc` accepts a capacity of one. `mpsc` cannot, and the reason is arithmetic rather than taste. Its slot
sequence distinguishes three states by counting -- `pos` is free, `pos + 1` is published, `pos + capacity`
is free again on the next lap -- and when `capacity == 1` the second and third are the *same number*. A
producer would read the sequence of the item it had just pushed, conclude the slot was free, and overwrite
an item the consumer had not read.

**It is reported, not worked around.** The obvious workaround -- allocate two slots and refuse the second
-- reintroduces a load of the consumer's position on the producer's hot path, which is precisely the cost
the sequence protocol exists to avoid, and it would impose that cost on *every* queue in order to serve a
capacity of one. A caller that genuinely wants a one-item handoff wants `spsc`, which represents it
exactly.

The consequence for the error type is small and was anticipated: `CapacityError` already carried a
`max_valid` on the argument that a bound "follows from how a shape represents its positions", and it now
carries a `min_valid` for the same reason. The suggestion methods respect it, so `bounded::<T>(1)` on an
`mpsc` reports `next_valid() == Some(2)` rather than a correction that would itself be refused.

Each shape names its own minimum as a documented constant next to the code that needs it, rather than
passing a bare literal, so the number is never separated from the reason for it.

## D-13: the arming protocol is stated once, and shapes bind to it

The blocking receive loop is not glue around [D-9](#d-9) -- it *is* D-9, executed: drain, arm, check for
disconnection, and wait only if arming blessed it. Every step is load-bearing and the order is the whole
correctness argument.

So it lives in [`blocking.rs`](src/blocking.rs), and a shape gains `recv` and `recv_timeout` by
implementing a crate-private `Parked` trait. A second shape spelling the loop out again would be a second
copy of a rule, free to drift, and -- the failure mode that actually bites -- free to *look* verified while
only the copy was tested. This crate has already paid for that once: the first lost-wakeup proof exercised
a hand-written duplicate of `Consumer::arm` and was structurally incapable of noticing the real `arm`
being reversed. The `ARM_RACE` hook is shared for the same reason.

`Parked` is deliberately *not* one of the public capability traits. The public traits say what a caller may
ask of a queue; `Parked` says what the blocking loop needs from one, and the difference shows in `finish`,
whose contract is a precondition no external caller can check.

## D-14: mpsc arms on readiness, not on emptiness

`Consumer::arm` must answer "is it safe to park?", and for `mpsc` that is not the same question as "is the
queue empty". They disagree over a slot a producer has claimed but not yet published, and the disagreement
matters in both directions:

- **`len` says non-empty**, because it counts the claim. Arming on that would refuse to bless the wait, and
  the consumer would spin -- calling `pop`, getting `None`, re-arming, getting `false` -- until the
  producer was rescheduled. Correct, and a burnt core.
- **Readiness says nothing is takeable**, so the consumer parks. That is safe precisely because the
  producer's publishing release store is followed by a signal, so the wakeup is guaranteed to arrive.

Arming therefore asks `Shared::has_ready_item`, which is the exact question `pop` answers: is the slot at
the head position published? `len` keeps its cheaper definition and its documented over-count, because it
is a metric rather than a control-flow input.

This also places the `SeqCst` pairing from D-9 correctly for this shape: the producer stores the slot's
sequence and then loads the doorbell state, while the consumer stores the doorbell state and then loads
that same sequence. It is the same store-buffer shape, over the same two fences, with a different pair of
locations.
