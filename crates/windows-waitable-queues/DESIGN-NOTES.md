# Design notes: windows-waitable-queues (Tier 1)

This crate is a skeleton. This file records the decisions its code will be built against, taken during
the 2026-08-30 design session and transcribed here so they steer the work rather than sitting in a
session record nothing is obliged to read. The work itself is tracked in
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
| <a id="d-9"></a>D-9 | **Without a lock, the reset is made inseparable from the observation by ordering: clear first, then re-check, and never wait if the re-check finds anything.** `Consumer::arm` is that step. The natural order -- check, then clear -- is the lost wakeup, and is asserted to hang by a deliberate sabotage rather than argued to be wrong. |
| <a id="d-6"></a>D-6 | **Overflow fails or reserves, and never overwrites.** For telemetry an overwritten entry is a lost sample; for an I/O submission it is a lost operation, and the two must not share a policy knob. |
| <a id="d-7"></a>D-7 | **Shapes are plain modules, not Cargo features, until compile time justifies otherwise.** Two features are four configurations to test, against a benefit dead-code elimination already provides. |
| <a id="d-8"></a>D-8 | **Published, and the obligation is accepted deliberately.** Unlike `windows-guard-alloc`, this is general-purpose and its first consumer is not its only plausible one. |

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
check that decides to wait, which is why `arm` creates it rather than assuming a caller did. Making the
initial state agree with the queue at creation time would not fix this and was rejected: doing it
race-free needs sequential consistency on both the event pointer and the queue position, which is a
`SeqCst` fence on the producer's hot path to close a hole the re-check already closes for free.

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
