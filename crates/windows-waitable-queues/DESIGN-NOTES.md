# Design notes: windows-waitable-queues (Tier 1)

This file records the decisions this crate's code is built against. D-1 to D-9 were taken during the
2026-08-30 design session and transcribed here so they steer the work rather than sitting in a session
record nothing is obliged to read; D-10 onwards were taken while building the shapes those decisions
called for, and record what the building settled or corrected. The work itself is tracked in
CHECKLIST-io-domains.md at the workspace root, because it spans several
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
| <a id="d-5"></a>D-5 | **The doorbell is level state owned by the queue: never unsignalled while the consumer has something to observe.** One-sided on purpose -- it may be signalled with nothing there, because the event stays set after the last take until the consumer's `arm()` clears it, and a late signal may arrive after a drain. A wake is a hint, never a proof; the guarantee is that a wake is never missing. The **reset** must not be separable from the observation that there is nothing to take; the **signal** may be. Manual-reset, and created lazily. Realized without a lock by [D-9](#d-9). |
| <a id="d-9"></a>D-9 | **Without a lock, the reset is made inseparable from the observation by two things: ordering (clear, then re-check, and never wait if the re-check finds anything) and a `SeqCst` fence on each side.** `Consumer::arm` is the ordering step; the fences defeat the store-buffer hazard that ordering alone leaves open. The natural order -- check, then clear -- is asserted to hang by deliberate sabotage; the fences are beyond any test's reach and are M31.6's target. **Amended: this decision originally claimed the ordering alone sufficed.** |
| <a id="d-6"></a>D-6 | **Overflow fails or reserves, and never overwrites.** For telemetry an overwritten entry is a lost sample; for an I/O submission it is a lost operation, and the two must not share a policy knob. |
| <a id="d-7"></a>D-7 | **Shapes are plain modules, not Cargo features, until compile time justifies otherwise.** Two features are four configurations to test, against a benefit dead-code elimination already provides. |
| <a id="d-8"></a>D-8 | **Published, and the obligation is accepted deliberately.** Unlike `windows-guard-alloc`, this is general-purpose and its first consumer is not its only plausible one. |
| <a id="d-10"></a>D-10 | **The multi-producer shape is Vyukov's bounded array queue: a sequence number per slot, claimed by a compare-and-swap on the tail and published by a release store.** The sequence is what lets the consumer tell a *claimed* slot from a *written* one, which a plain fetch-and-add cannot. Lock-free rather than wait-free, bounded by construction, and no allocation after the constructor. |
| <a id="d-11"></a>D-11 | **The capability traits shipped with this second shape, and the signatures `spsc` wrote down in advance held unchanged.** That is [D-3](#d-3)'s check actually being run rather than assumed. The load-bearing choice was `push(&self)`: `&mut self` would have been sound for one producer and would have made the trait unimplementable by this one. |
| <a id="d-12"></a>D-12 | **A shape's *minimum* capacity belongs to the shape, not to the crate, and `slotwise_mpsc`'s is two.** One slot cannot encode three states when the lap stride is the capacity, so "published at `p`" and "free again at `p + capacity`" collide. Reported through `CapacityError` rather than worked around, because every available workaround puts a load back on the producer's hot path for every queue in order to serve a capacity of one. |
| <a id="d-13"></a>D-13 | **The arming protocol is written once, in [`blocking.rs`](src/blocking.rs), and a shape binds to it by implementing a crate-private `Parked` trait.** The blocking receive loop *is* [D-9](#d-9), not glue around it; a second shape spelling it out again would be a second copy of a rule -- the exact mistake this crate has already paid for once. |
| <a id="d-14"></a>D-14 | **`slotwise_mpsc`'s arming asks "would `pop` find something", not "is `len` zero".** The two disagree over a slot a producer has claimed but not published, and only the first answer lets the consumer park on it instead of spinning until that producer is rescheduled. |
| <a id="d-15"></a>D-15 | **`Doorbell::clear` resets the event *before* clearing the flag that mirrors it, and the original order was a lost wakeup.** A producer signalling between the two lines set the flag and issued a real `SetEvent`; the `ResetEvent` that followed erased the signal and left the flag set, wedging the doorbell dark while it claimed to be lit. **Amends [D-9](#d-9)**, whose "there is no third case" holds only for a queue whose emptiness is one position comparison. |
| <a id="d-16"></a>D-16 | **Its cost premise is falsified by [D-26](#d-26); the conclusion stands on capability instead -- see [D-29](#d-29).** Reservation is a capability a shape may lack, so the reserving multi-producer queue ships as a peer of `slotwise_mpsc` rather than replacing it. Honouring a reservation requires counting free slots, which requires the consumer's position -- a single shared line `slotwise_mpsc`'s push deliberately never reads. The original rationale added that this made reserving the *more expensive* shape and that both should ship rather than charge every caller for it; measurement reversed that, and the split is now justified by the capability alone. **Amends [D-6](#d-6)**, which assumed one queue would carry every policy. |
| <a id="d-17"></a>D-17 | **The reservation count and the claim position live in one word, because a check-and-claim over both must be a single atomic operation.** Two atomics cannot be made correct with any amount of fencing: the pushing producer is load-then-store and the reserving one store-then-load, so the Dekker argument does not apply and both can miss each other. The 32/32 split is forced by the arithmetic, and caps this shape at 2^31 items. |
| <a id="d-18"></a>D-18 | **Superseded by [D-37](#d-37), which adopts a 128-bit compare-and-swap for a separate wide shape.** Retained because its analysis of the *costs* is still correct and D-37 depends on it; what changed is that those costs are now paid by a **separate shape** rather than imposed on this one. **Originally: a 128-bit compare-and-swap is refused.** **Amended once before being superseded, because three of the four reasons originally given were wrong or incomplete, and the decisive one was missing.** It would *not* lift the cap "and nothing else": a 64-bit position also collapses SH-14.1's ABA recurrence, which was unknown when this was written. It is *not* outside the x86-64 baseline -- `rustc 1.98.0` emits `target_feature="cmpxchg16b"` for `x86_64-pc-windows-msvc`, so there is no floor to raise and no runtime detection to pay. What stands is the dependency (`AtomicU128` is still unstable, rust-lang/rust#99069) and, decisively, that **`i686-pc-windows-msvc` has no 128-bit atomic at all**: adopting this is not "widen the word" but "widen the word *and* drop 32-bit support". Revisit for a tagged pointer, or if 32-bit support is dropped for other reasons -- not before. |
| <a id="d-19"></a>D-19 | **The coalesced loss latch is deliberately not generalised from the file watcher.** Coalescing there is sound because a desync is *idempotent* -- two mean the same as one, and the answer to both is a re-scan. A queue of arbitrary `T` has no such property, so what generalises is a loss *count*, which is M31.4's observability rather than a policy. |
| <a id="d-20"></a>D-20 | **Undrained items are handed to a caller-supplied sink at teardown, and the sink is chosen at construction because `Drop` has nowhere to hand them back to.** Without one they are destroyed on whichever thread released the last handle -- which may be a pool callback that must not block, and closing a handle to a dead network path can block for a long time. The default is unchanged; what changes is that it is now a named choice. |
| <a id="d-21"></a>D-21 | **A panicking disposal sink is caught and the teardown walk continues.** The sink is caller code inside a destructor: a panic escaping it abandons every item behind it -- the exact handles the mechanism exists to account for -- and during an unwind aborts the process. Catching declines to turn a caller's bug into a much larger one. |
| <a id="d-22"></a>D-22 | **No `into_remaining`, because it would not close the hole and `drain` already covers what it would do.** A consumer can take everything available, but a producer may push afterwards, so an orderly drain covers only the orderly path. The last handle to drop is the only place that sees every survivor. |
| <a id="d-23"></a>D-23 | **High-water tracking is opt-in at construction; refusals and doorbell rings are always on.** The difference is where each can be paid for: refusals sit on the failure path and rings on a path that already costs a syscall, but a peak has to observe *every* change -- and on `slotwise_mpsc` that means the producer reading the consumer's position, the shared line [D-16](#d-16) built a separate shape to avoid. Untracked reports `None`, not `0`. |
| <a id="d-24"></a>D-24 | **Counting the doorbell's rings turns the skip optimisation into part of the observable contract, and that is the point rather than a side effect.** R9 asks for the count precisely so "disabling the skip must change the number" -- so the sabotage entry for removing the skip changed from a control expecting `survives` to a defect expecting `caught`. An optimisation nobody can measure is an assumption. |
| <a id="d-25"></a>D-25 | **`Observable` deliberately does not restate depth.** [D-2](#d-2)'s sketch listed it, but `Bounded::len` already reports it from positions the queue keeps anyway. Naming it twice would give one number two spellings and two places to drift. What belongs on `Observable` is only what must be *accumulated*. |
| <a id="d-26"></a>D-26 | **Measured: the tail claim contends badly, and `reserving_mpsc` is up to 4x FASTER than `slotwise_mpsc` under contention -- the opposite of what [D-16](#d-16) assumed.** Aggregate throughput *falls* as producers are added, for both shapes and far more than a bare contended atomic explains. D-16's premise, that reading the consumer's position makes the reserving shape the expensive one, is falsified everywhere except a single producer with a live consumer. |
| <a id="d-27"></a>D-27 | **The gap is intrinsic to Vyukov's sequence protocol, not a fixable flaw in `slotwise_mpsc`'s retry loop.** Its producer must read a slot's sequence *before* claiming, and that slot marches through memory as the tail advances while other producers write it. Padding slots onto their own cache lines was tested and rejected: it recovers about a fifth at eight producers, for four times the memory, and leaves the shape still 2.8x slower. |
| <a id="d-28"></a>D-28 | **Amended -- the blanket rejection is withdrawn; the verdict depends on thread placement, and the open question is queued as CHECKLIST-io-domains.md M-inf.4.** Caching the peer's index was measured, and it engaged as designed. It cost ~1.8x on x64 with the threads across cores, and *won* 17x on ARM64 and 1.8x on x64 SMT siblings. Batch depth decides the sign, and batch depth is set by where the two threads are scheduled -- not by the architecture and not by our code. A prefetch-only "warming" control changed nothing on any host. |
| <a id="d-29"></a>D-29 | **Both multi-producer shapes ship. The crate publishes what it measured and declines to choose for the caller.** [D-26](#d-26) falsified [D-16](#d-16)'s cost premise, which reopened merge-or-delete; the answer is neither. Vyukov's sequence protocol and the head-based one are independently researched designs, both in production use, and our own workload having settled which *we* want is not evidence about anyone else's. Deleting a shape because no visible consumer wants it is what PLATFORM INTEGRITY forbids. What the crate owes instead is the data and, through `probe-core-affinity`, the means to gather it on the caller's own hardware. |
| <a id="d-30"></a>D-30 | **Both MPSC shapes are qualified by name; neither is `mpsc`.** A bare `mpsc` beside `reserving_mpsc` makes one canonical by implication, which contradicts this crate's own "no shape is the canonical one" and, after [D-29](#d-29), is simply false. `slotwise_mpsc` names its claim protocol -- it claims slot by slot, with no shared counter -- and avoids the reading `sequence_mpsc` invites, that it alone preserves FIFO order when both shapes do. Renamed before first publish, where it is free. |
| <a id="d-31"></a>D-31 | **0.1.0 ships without machine-checked memory orderings, and says so in its own documentation.** Model-checking gates 1.0, not 0.1.0. It would close the *demonstrated* gap -- a weakened `Acquire` survives the whole suite -- but not the dangerous one: it cannot model `SetEvent`/`ResetEvent`, so it cannot cover the doorbell, and [D-15](#d-15)'s lost wakeup, the only ordering bug this crate has had, was found by sabotage instead. The risk it addresses is mostly regression risk, which is lowest before there are consumers. The disclosure, not the deferral, is the decision. |
| <a id="d-32"></a>D-32 | **`Reserving::Reservation<'a>` gains a bound, before the crate publishes.** The associated type is currently unbounded, so a caller generic over the trait can claim a slot and drop it but never redeem it -- the trait cannot express the operation it exists for. Both implementors already have identical `send` and `is_disconnected` signatures, so the bound is additive; adding it after publication is a breaking change to every implementor. Done as SH-1.5: the [`Claim`](src/traits.rs) trait carries `send` and `is_disconnected`, and both reservation types implement it as forwarders. `Claim` must be in scope to call those methods on a claim whose concrete type the caller has not named, which is why it is re-exported at the crate root. |
| <a id="d-33"></a>D-33 | **`PushError` is `#[non_exhaustive]`, and the one-directional doorbell is disclosed rather than fixed before 0.1.0.** The receive-side errors already carried the attribute and the send side lacked it by omission; adding it after publication is itself breaking, so it is taken now while the crate has no external consumers. Whether a producer can *wait* for room stays open as M32.3 -- it is additive, so it does not gate the release -- but the absence is stated in both the crate docs and the README, because `crossbeam-channel`'s `send` blocks and a reader arriving from it will assume this one does too. |
| <a id="d-34"></a>D-34 | **Every bounded queue surveyed is ABA-safe for one of two reasons, and this crate's `reserving_mpsc` has neither.** Either the claim counter is a whole machine word, so recurrence is unreachable -- crossbeam, concurrent-queue, thingbuf, Vyukov, SCQ's `Head`/`Tail` -- or the authorizing compare-exchange is moved onto the cell, so the decision and the write are validated together (CRQ, SCQ). Ours packs the position into a 32-bit *subfield* and authorizes with an exchange that does not cover the separately-read `head`. Nikolaev (DISC 2019, section 3) states the width assumption the field relies on and states it for **CPU-word** width, which a subfield does not satisfy; DPDK's `rte_ring` is the same protocol as ours and its published justification covers modular arithmetic only. The generalisation -- ours, unstated in any source -- is that **the atomic operation authorizing the write must cover everything the decision depended on.** Survey in [DESIGN-SESSION-2026-09-02](design-sessions/DESIGN-SESSION-2026-09-02-claim-protocol-prior-art.md); the fix is M15 in CHECKLIST-ship-topology-and-queues.md. |
| <a id="d-35"></a>D-35 | **Measured: the permit claim is 2.7x faster than `reserving_mpsc` at 16-32 producers, and 1.45x slower at one.** The safer claim is also the faster one everywhere contention exists, which was not the expected result -- it touches *two* shared lines where the shipping shape touches one plus a read, and [D-26](#d-26) had established that the shared line is what collapses. The mechanism is that both of its operations are unconditional read-modify-writes that never retry, where the shipping shape's compare-exchange retries once per lost race; the retries dominate long before the second line does. It is the only shape measured that gets *faster* per push as producers are added (42.8 ns at two to 19.5 at thirty-two) and the only one that stays within 1.5x of a bare contended `fetch_add`. **This decides the shape of the fix but not the fix**: the drained regime's refusal counts differ by orders of magnitude in a way this harness cannot attribute, which SH-15.5.1 exists to settle before SH-15.6 adopts anything. |
| <a id="d-36"></a>D-36 | **Superseded by [D-41](#d-41): the hazard is now a layout choice, not a defect that must ship.** The reasoning below stands as the record of why it was right to disclose rather than delay while the only known fix was the claim-protocol replacement. **0.1.0 ships SH-14.1 disclosed rather than fixed, and the disclosure is a release blocker.** Following [D-31](#d-31)'s principle -- the disclosure, not the deferral, is the decision -- because the fix is a claim-protocol replacement ([D-35](#d-35)) whose adoption is still gated on an open question, and holding the release for it would trade a *documented* hazard for an undocumented rush. **The two gaps are not equally forgiving and the text says so**: an unverified ordering is a risk of a bug, this is a known one with a computed exposure, and its failure mode is silent -- no error, panic, or counter -- so a caller can neither detect nor mitigate it. That is precisely why it may not ship in silence. Stated in the crate docs, the README, and the shape's own module docs, each leading with **"on every target, not only 32-bit ones"**, because the natural spelling "32-bit position" invites the opposite reading and SH-6.1 already had to be corrected for exactly that. The shape-selection guidance in both documents was also amended: it previously said "start with `reserving_mpsc`" with no caveat, pointing callers at the hazardous shape by default. |
| <a id="d-37"></a>D-37 | **Partly superseded by [D-41](#d-41): the wide word ships as a *layout* behind the non-default `dwcas` feature, not as a separate `reserving_mpsc_wide` shape, and the gate is the feature rather than the target.** What stands is the reasoning below about `portable-atomic`: `default-features = false` is load-bearing, because with defaults on it silently substitutes a global lock, and D-7's burden of proof is discharged rather than waived. What does not is the shape's name and the premise that the narrow word must keep SH-14.1 -- re-apportioning the narrow word removes the exposure for free, so the wide word is no longer the only way out. **The reserving claim word ships in two widths: the narrow one on every target, the wide one only where a 128-bit exchange is genuinely lock-free.** `reserving_mpsc` keeps its packed 64-bit word, keeps SH-14.1's hazard, keeps [D-36](#d-36)'s warnings, and is **never silently swapped** for the wide shape on targets that could host one -- a contract that changes with the target is what PLATFORM INTEGRITY rule 2 forbids, and a caller who read "2^32" must get 2^32. `reserving_mpsc_wide` is the same protocol with a `u128` word split 64/64: recurrence needs 2^64 pushes, and the capacity ceiling rises to 2^62. **The gate is one line of `Cargo.toml`: `default-features = false`.** Measured, not designed -- with the default feature set `portable-atomic` compiles on i686 and silently substitutes a global lock, but with defaults off `AtomicU128` **does not exist** there (`no AtomicU128 in the root`), nor on x86_64 built without `cmpxchg16b`. It exists exactly where a native lock-free exchange is guaranteed at compile time, so the `use` statement is the gate and it fails loudly. A `cfg(target_has_atomic = "128")` would be the *wrong* gate -- it is emitted even with `cmpxchg16b` disabled -- and a `const` assertion on `is_always_lock_free()`, though genuinely const-evaluable, is redundant where the type exists and unreachable where it does not. That is the standard SH-14.2 already set when it probed i686 to confirm `AtomicU64` was lock-free before widening `slotwise_mpsc`, recording that a hidden mutex "would have made this a bad trade". [D-7](#d-7)'s burden of proof for adding a Cargo feature is **discharged, not waived**: D-7 rejected feature-gating because the only benefit was compile time, and the cost here is a third-party dependency, which dead-code elimination does not remove from `Cargo.lock` or from an auditor's review. |
| <a id="d-38"></a>D-38 | **One atomic, one discipline: an atomic that carries any acquire/release operation has acquire/release on *every* operation, and a relaxed load is never mixed in.** A relaxed operation is still **atomic** -- indivisible, untorn, and free of data-race UB -- but it is *unordered*: it behaves like a plain load or store with respect to placement, unanchored relative to the ordered operations on the same object and free to be moved by the optimizer or the processor. It is not pinned to its textual site, so reasoning about it in statement order is nonsense. **The two axes are independent, and conflating them is the mistake in both directions:** relaxed does not mean "no guarantees" (see [D-40](#d-40) -- the atomicity is often the whole point), and it does not mean "ordered but weakly". The failure mode is that it usually does what the source appears to say, until a change of code generator or a weaker processor makes it not; on x86-64 TSO a decorative `Acquire` and a `Relaxed` load emit near-identical code, so a test suite on this host cannot see the difference at all -- which is the same blindness [D-31](#d-31) measured. **When the two resolutions differ, promote the load.** The exception is a reference count ([D-39](#d-39)), and it is an exception for a stated reason rather than by convention. |
| <a id="d-39"></a>D-39 | **The reference counts keep a relaxed increment against an `AcqRel` decrement, and that is the one sanctioned departure from [D-38](#d-38).** It is sanctioned because *no dependent memory is read on the strength of the relaxed increment*: a thread incrementing `producers` already holds a handle, so it needs no edge to learn the object exists, and the cache-coherence effect an acquire would buy is not required until the count reaches zero -- at which point the `AcqRel` decrement supplies it. That is the same argument `std::sync::Arc` makes, and it is a property of what the count is used for, not a general licence. A count whose value ever decided whether to *dereference* something would not qualify. |
| <a id="d-40"></a>D-40 | **A relaxed atomic is chosen for its *atomicity*, and dropping to a plain field is never the way to "simplify" one.** [D-38](#d-38) says a relaxed operation is unordered; it is still indivisible, and that is frequently the entire reason the field is an atomic at all. Without it the implementation may synthesise a wide access out of narrower ones and observe a **torn** value, and concurrent access to a non-atomic field is a data race and therefore UB regardless. `reserving_mpsc`'s claim word is the worked example: it packs `reserved` and `position` into one `u64` and is uniformly relaxed ([D-38](#d-38)), yet a torn read would yield a pair that never existed as a state and would break the compare-and-swap protocol outright. On `i686-pc-windows-msvc` -- which [D-18](#d-18) deliberately keeps supported -- that load must be a `cmpxchg8b` or an 8-byte SSE load, *more* expensive than the two `mov`s a plain `u64` would get, and the compiler is obliged to pay it for exactly this reason. **Relaxed is a statement about ordering only; it is never a step toward removing the atomic.** |
| <a id="d-41"></a>D-41 | **The claim word's apportionment is a caller's choice, and the recurrence behind SH-14.1 is a number the caller sets rather than one this crate imposes.** Supersedes [D-36](#d-36), whose premise was that the only fix was the [D-35](#d-35) claim-protocol replacement, gated on an open question -- so disclosing beat delaying. That premise was false, and measurement is what showed it: the 32/32 split followed from requiring the reservation half to hold the *entire capacity*, because every slot could be reserved at once. Capping outstanding reservations instead leaves the capacity bounded only by the ring, and the position is free to take 48 or 56 bits. `Balanced` (32/32), `Enduring` (16/48) and `Perpetual` (8/56) issue the **same** `lock cmpxchg` on the same `u64`, differing only in shift constants, and measured indistinguishable outside noise -- so the recurrence moves from about 37 seconds to about 20 years for no throughput and no dependency. The reservation half was the wrong half to spend bits on: it held 2^32 where the real bound is however many producers are mid-send. `Wide` (64/64 over a `u128`) is offered behind the non-default `dwcas` feature, because it is the one thing here that costs a third-party crate -- the standard library has no 128-bit atomic -- and it measured 2-3x slower on the claim; it buys a guarantee rather than a lifetime argument. **The default stays `Balanced`** so introducing the choice changed no existing caller's behaviour, and it is documented as *not* the recommended layout: leaving it because it is the status quo would preserve the hazard by inertia. |

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

The invariant is one sentence, and it is deliberately one-sided: **the event is never unsignalled
while the consumer has something to observe.** It is *level* state -- a function of the queue's
contents -- rather than a record of edges, which is why it is manual-reset.

The converse does not hold, and stating it as "signalled exactly when" -- which an earlier wording
of this section and of [D-5](#d-5) both did -- promises more than the crate delivers, in a
paragraph immediately followed by the two bullets that contradict it. The event stays signalled
after the last item is taken until the consumer's own `arm()` clears it, and a late signal can
arrive after the consumer has already drained. So a wake is a **hint that there may be something**,
never a proof that there is; what the crate guarantees is that a wake is never *missing*. That is
why the consumer protocol is pop, `arm()`, re-check, and not "wait, then take".

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
signals a doorbell that is no longer about to be reset.

**"There is no third case" is what this decision originally said next, and it was wrong** -- see
[D-15](#d-15). It holds for `spsc`, where one producer and one position mean that *any* push before the
clear makes the check find something. It fails for `slotwise_mpsc`, where the check asks whether the *head* slot
is published: a producer publishing at a later position before the clear is the third case, invisible to
the check. The remedy is in `Doorbell::clear` rather than here, because what that case needs is not a
better check but a doorbell that is guaranteed able to ring again once the clear returns.

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
whole suite green, and no entry in [`sabotage.json`](sabotage.json) can express it, because the defect is a fact about
the memory model rather than an interleaving a scheduler can be coaxed into producing. It is the named
target of the `loom` work in CHECKLIST-io-domains.md item M31.6.

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

## D-10: the slot-wise MPSC shape is Vyukov's bounded array queue

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
`slotwise_mpsc` is a lock-free array queue with a per-slot state machine and no structural resemblance to a
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

## D-12: the minimum capacity belongs to the shape, and `slotwise_mpsc`'s is two

`spsc` accepts a capacity of one. `slotwise_mpsc` cannot, and the reason is arithmetic rather than taste. Its slot
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
`slotwise_mpsc` reports `next_valid() == Some(2)` rather than a correction that would itself be refused.

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

## D-14: ``slotwise_mpsc`` arms on readiness, not on emptiness

`Consumer::arm` must answer "is it safe to park?", and for `slotwise_mpsc` that is not the same question as "is the
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

## D-15: the clear order, and the assumption that hid a lost wakeup

`Doorbell::clear` has two lines: reset the kernel event, and clear the `AtomicBool` that mirrors it so a
redundant `signal` can skip its syscall. **They originally ran flag-first, and that order is a permanent
hang.** A producer signalling between them finds a clear flag, sets it, and issues a real `SetEvent`; the
`ResetEvent` that follows erases that signal and leaves the flag set. The doorbell is then dark while
claiming to be lit, so every later `signal` skips, and a consumer parked on it never wakes.

The flag is allowed to lie in exactly one direction -- claiming lit while the `SetEvent` has not landed
yet, which costs a skipped *redundant* signal. The order above produced the opposite lie, which costs the
one signal that mattered.

**Why it survived review and a sabotage sweep.** The original argument was explicit and looks airtight:
the racing producer publishes *before* it signals, so the caller's re-check sees the item and does not
wait. It is sound -- for a queue whose re-check is guaranteed to see anything any producer published.
`spsc` is such a queue: one producer, one tail, and `is_empty` covers every push. So the argument was
tested against the only shape that could not falsify it, and it was written down as a general rule.

`slotwise_mpsc` falsifies it. Its re-check asks whether the **head** slot is published ([D-14](#d-14)), so a
producer publishing at a later position is invisible to it. The consumer parks in exactly the wedged
state, the producer holding the head publishes, its `signal` is skipped, and the queue hangs with an item
sitting in it.

**How it was found, which is the part worth keeping.** Not by review, and not by the test suite: the
suite passed 120 tests in 0.28 s, six runs in a row. It was found because the sabotage harness refuses to
sweep against a red baseline, and its *baseline* run -- the one that exists only to prove the suite is
green before any defect is injected -- hung once in
`slotwise_mpsc::tests::many_producers_deliver_every_item_exactly_once`. A single unreproducible hang is exactly
the finding it is tempting to dismiss as a slow machine, and the crate's own sabotage documentation
already says not to: "a flaky sabotage is a finding, not noise". The same applies to a flaky baseline.

**The fix moves the guarantee from the caller to the type.** With the event reset first, the invariant is
a property of the doorbell rather than an obligation on whoever calls it: *once `clear` returns, the flag
is false, so the next `signal` cannot be skipped.* A producer signalling inside the window may still be
skipped, but it published before it signalled and therefore before the flag store, so the caller's
re-check observes whatever that publication made observable; and any producer that publishes after the
re-check finds the flag already false and rings for real. No caller has to reason about it, which is the
point -- the previous arrangement required every future shape to have a re-check strong enough to cover
the window, and no signature said so.

**It is asserted deterministically, at the layer that owns it.** `race_hooks::CLEAR` fires inside the
real `clear`, between its two lines, and a test signals from there on one thread. The assertion is not
about the state immediately afterwards -- both orders leave the event dark -- but about what a consumer
depends on next: `signal` must still be able to ring. Reversed, the test fails every run; a sabotage
entry keeps it that way. A control with an empty window sits beside it, so the test cannot pass by
`clear` simply never leaving the doorbell ringable.

**Two temptations refused.** Making `slotwise_mpsc` arm on `len` instead of readiness would also have masked this,
by restoring the property that any push makes the re-check find something -- but it would have left the
doorbell able to reach the inconsistent state, waiting for the next shape, and it would have cost the
consumer a spin whenever a claim was in flight. Adding a lock around the two lines would have fixed it
and thrown away the reason the flag exists.

## D-16: reservation is a capability, so the reserving queue is a peer and not a replacement

[D-6](#d-6) said overflow "fails or reserves, and never overwrites", and quietly assumed one queue would
carry both policies. Building the second one showed that assumption was wrong, and why.

**The cost claim in this section is falsified; the structural claim is not.** Read what follows as an
account of *why the two shapes differ*, which remains correct, and not as an account of which is
cheaper, which [D-26](#d-26) reversed. [D-29](#d-29) records what the split rests on now.

**Honouring a reservation costs the producer something on every push, including the pushes that never
reserve anything.** `slotwise_mpsc`'s producer never reads the consumer's position: it asks the slot's own
sequence number "are you free?", and those are spread across the slot array, so producers working at
different positions touch different cache lines. Avoiding a single shared position is not incidental to
Vyukov's design; it is most of the point of it.

A reservation cannot be answered from that question. "Is this slot free" does not say **how many** slots
remain, and withholding one from the best-effort path requires exactly that count -- which requires the
consumer's position, on one line every thread in the system touches.

So the choice was: pay that on every `slotwise_mpsc` push, or ship two shapes. Two shapes, for three reasons:

- **The cost falls on the shape M31.5 exists to measure.** Degrading
  `slotwise_mpsc`'s push before the contention benchmark runs would corrupt the measurement that decides whether
  the deferred shapes are needed at all.
- **The crate is built for this.** It is named in the plural, [D-7](#d-7) makes shapes plain modules, and
  [D-4](#d-4) already has shapes differing in what they can do. A third one is the pattern working, not
  an exception to it.
- **It is [D-2](#d-2)'s argument reaching its sharpest case.** `slotwise_mpsc` does not implement `Reserving`
  because it genuinely *cannot*, not because nobody got round to it -- which is exactly the situation
  narrow traits were chosen for. A fat trait would have forced the cost on both shapes or excluded
  reservation from the contract entirely.

**The alternative shape considered and refused was a permit counter** -- one atomic that both producers
and the consumer read-modify-write, acquiring on push and releasing on pop. It is correct and simpler to
read, and it was rejected because it puts a second contended read-modify-write on the push path where the
packed word puts one shared *load*. Its only advantage is preserving the crate-wide capacity ceiling, and
[D-17](#d-17) explains why that ceiling is unreachable anyway.

**The merge-or-delete decision is deferred to M31.5, deliberately and with a trigger.** If the benchmark
shows the shared-line read costs little at realistic contention, `slotwise_mpsc` and `reserving_mpsc` should merge
and the plain one should go. If it shows the read is expensive, both stay. What must not happen is the
duplicated path becoming permanent because nobody circled back, so the decision is recorded as an item on
M31.5 rather than as an intention here.

## D-17: the reservation count and the claim position share one word

**The obvious implementation is broken, and it is worth writing down why, because the brokenness is not
visible from reading either side on its own.** With the count in its own atomic:

1. A pushing producer reads the count, sees room, and claims the position.
2. A reserving producer increments the count, reads the position, sees room, and grants.

Each read before the other's write. The queue now owes a slot that does not exist, and the guarantee the
whole feature rests on is gone.

**Sequentially consistent fences do not close this**, which is the part that surprises -- they *do* close
the superficially identical hazard in [D-9](#d-9). The Dekker argument needs store-then-load on both
sides. Here the pushing producer is **load**-then-store: it reads the count and then writes the position.
Writing the four operations into a single total order, `L_push < S_reserve < L_reserve < S_push` is
consistent with every side's program order, so both sides missing each other is permitted and no fence
forbids it.

Two independent claimants on one resource must synchronise on **one location**. So the count and the
position become one location: a single `AtomicU64`, low 32 bits the position, high 32 the count. Every
operation that changes either changes both, with one compare-and-swap.

Three consequences fall out, and all three are improvements:

- **Redeeming is one exchange** that decrements the count as it advances the position, so
  `occupied + reserved` -- the quantity the invariant is about -- is never momentarily wrong.
- **A racing `reserve` and `push` cannot both win.** The loser's exchange fails and it re-reads, which is
  the ordinary lock-free retry rather than a special case.
- **The producer stops needing the slot sequence for the "free" direction**, because it now reads the
  consumer's position anyway. So `reserving_mpsc`'s `pop` is one store shorter than `slotwise_mpsc`'s: nothing
  writes a "free again" sequence.

**The 32/32 split is forced, not chosen.** A position of `b` bits keeps a wrapping difference unambiguous
only up to `2^(b-1)`; the count can reach the capacity, so it needs `b` bits too; `b + b = 64` gives
`b = 32`. There is no cleverer division of the word, and the resulting ceiling is 2^31 items -- a ring
this shape allocates in full at construction, so at eight bytes an item it is already 17 GB.

That ceiling is reported through `CapacityError`'s `max_valid`, which [D-12](#d-12) had already made a
property of the shape rather than of the crate. D-12 introduced that for the *minimum* and argued the
maximum worked the same way; this is that argument being cashed.

**The invariants the packing depends on are `const` assertions, not tests**, because they are facts about
constants: a test can only report after the fact, on a build somebody chose to run. Worth recording that
the first version of those assertions was *tautological* -- it asserted that `BOUNDS_MAX` equalled its own
definition -- and widening the position to 40 bits sailed straight past it while silently narrowing the
count's field to 24 bits, which is the way the packing actually breaks. The assertions now name the
constraint that binds: the count's half must be wide enough to hold the whole capacity.

## D-18: a 128-bit compare-and-swap is refused

**Superseded by [D-37](#d-37).** A 128-bit exchange is now adopted, but for a **separate wide
shape** rather than for this one: `reserving_mpsc` keeps its packed 64-bit word on every target, and
`reserving_mpsc_wide` is a peer beside it. Read this decision for the cost analysis, which D-37
depends on and does not repeat -- and note one correction it needs, below, that D-37's gate is built
around.

**The i686 failure is not a build failure, which is worse.** The amendment below says i686 "has no
128-bit atomic at all", which reads as "it will not compile". It compiles: `portable-atomic`'s
default `fallback` feature silently substitutes a **global lock**, so the queue keeps working and
stops being lock-free, contending with any unrelated user of the fallback in the same process. And
`target_has_atomic="128"` is emitted even with `cmpxchg16b` disabled -- verified on 1.98 -- so a cfg
gate does not catch it either. That silent degradation, not a compile error, is the real reason this
shape may not simply widen its word.
D-37 turns it back into a build failure by the simplest available means: depending on
`portable-atomic` with **`default-features = false`**, which withholds the `fallback` feature, so
`AtomicU128` does not exist at all on a target that cannot do it natively.

**Amended 2026-09-02. The refusal stands; almost none of its original reasoning does.** The first
version of this decision was written before SH-14.1
existed, and it asserted target facts that were never checked against the toolchain. Both faults are
corrected below, and the correction is kept rather than silently rewritten because the *shape* of the
error is instructive: a decision can reach the right outcome and still leave every reason a reader
would rely on wrong.

The natural question about [D-17](#d-17)'s packing is why not use `cmpxchg16b` (or `CASP` on aarch64)
and keep both halves full width.

### What was claimed, and what is actually true

**"It would lift the 2^31 cap and nothing else" -- wrong, and this is the substantive correction.**
A 64-bit position field would also collapse SH-14.1's ABA recurrence, which is a correctness hole and
not a capacity limit. The original decision denied the existence of what is now the option's main
benefit, purely because the hole had not yet been found. It remains true that a wider producer word
does nothing about *the cost that matters* -- free space is `capacity - (position - head) - reserved`,
`head` belongs to the consumer, and no width of producer-side exchange brings it into the producer's
word -- but "no performance benefit" was never the same claim as "no benefit".

**"It is not in the x86-64 baseline" -- false on the pinned toolchain.** Checked rather than assumed:
`rustc 1.98.0 --print cfg --target x86_64-pc-windows-msvc` emits `target_feature="cmpxchg16b"` and
`target_has_atomic="128"`. There is no target-feature floor to raise and no runtime detection to pay
on the push path. The original text reasoned from the generic x86-64 baseline and never checked the
*Windows* target, which enables the feature by default.

**"It is not even the same instruction on aarch64" -- true but not a cost.** `aarch64-pc-windows-msvc`
reports `target_has_atomic="128"` with no target feature required, because `ldxp`/`stxp` is ARMv8-A
baseline. That the instruction differs from x86-64's is what an atomics abstraction is for.

**"There is no usable `AtomicU128`" -- true, verified.** Still unstable (rust-lang/rust#99069); a
test compile on 1.98.0 fails. Reaching a double-width exchange from stable means adding
`portable-atomic` to a workspace whose only third-party dependency is `windows-sys`, on a crate that
is [published](#d-8). **This is the one original reason that survives.**

### The reason the decision actually rests on now

**`i686-pc-windows-msvc` has no 128-bit atomic at all** -- `rustc --print cfg` reports
`target_has_atomic="64"` and no `"128"`. So this option is not "widen the claim word"; it is "widen
the claim word **and** drop 32-bit support", which collapses
SH-14.3's option 1 into its option 4. Narrowing the
platform is the engineer's decision under the repository's platform-integrity rule, not something a
correctness fix may take in passing.

That is a stronger and simpler reason than the three it replaces, and it is the one to quote.

### When to revisit

For a **tagged pointer**, which is what the linked and sharded shapes parked in `M-inf.1` would need
-- or if 32-bit support is dropped for unrelated reasons, at which point the option becomes a live
candidate for SH-14.1 rather than a non-starter. Recorded so the question does not have to be
re-derived a third time.

## D-19: the coalesced loss latch does not generalise

[windows-file-watcher's queue](../windows-file-watcher/src/queue.rs) carries a third policy beside
fail-fast and reserve: a failed enqueue latches the affected `WatchId` in a set held *outside* the bounded
queue, where it coalesces, and is drained back in at the next successful enqueue. It is a good design and
this crate deliberately does not copy it.

**Coalescing is sound there because a desync is idempotent.** Two lost notifications for one subscription
mean the same thing as one -- the client must re-scan -- so collapsing them loses nothing, and that is
what makes the latch lossless despite being bounded by the number of subscriptions rather than by the
number of losses.

A queue of arbitrary `T` has no such property. There is no general way to collapse two lost `T`s into
one, and no general way to say what a client should do about them. What *does* generalise is the part
that does not depend on the payload: **a count of what was refused**, so loss is measured rather than
silent. That is observability, and it belongs to M31.4 rather than to the
overflow policy.

So this crate's answer to a full queue is: refuse and hand the item back, or hold a reservation so the
refusal cannot happen to the messages that cannot survive it. A caller whose payload *is* idempotent can
build the watcher's latch on top of the typed refusal, which is the right layer for a decision that
depends on what the payload means.

**Overwrite-oldest remains refused outright**, as [D-6](#d-6) said. `crossbeam`'s `force_push` makes an
`ArrayQueue` usable as a ring buffer, which is right for telemetry where an overwritten entry is a lost
sample. Here an entry may be an I/O submission, where it is a lost *operation*. The two must not share a
knob, because a knob invites a caller to pick the wrong one.

## D-20: teardown hands undrained items back, and the decision is made at construction

[R8](../../design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md) asks that
descriptors in flight at teardown be **accounted, not dropped**, "because some own handles, and their
disposal must be allowed to block". The 2026-08-27 namespace session states the same hazard concretely:
an async open's completion carries an owned handle, and closing one to a dead network path is exactly the
blocking operation the whole facility exists to keep off a caller's thread.

**The default answer to "who destroys the items nobody drained?" was bad in a way that is easy to miss.**
They were destroyed in place, inside the last `Arc` release -- so `T`'s destructor ran on whichever thread
happened to drop last. That thread is not knowable in advance and nobody chose it: it may be a thread-pool
callback that must not block, or a producer with no idea it was holding the last reference. Nothing told
the owner it had happened.

**`Drop` cannot be made to hand them back.** It takes `&mut self`, returns nothing, and cannot fail; by
the time it runs every handle is gone, so there is nobody left to return anything *to*. Whatever the queue
is going to do with those items, it has to have been told beforehand. That is the whole reason [`Disposal`]
is supplied at construction rather than asked for at teardown -- not ergonomics, but the shape of the only
place that sees every survivor.

So a queue built with a sink hands each survivor to it. The owner then decides where disposal happens: a
sink that moves items to a reaper thread keeps the blocking off the dropping thread entirely, while one
that disposes inline is perfectly fine when the dropping thread is allowed to block. Either way it is a
decision somebody made.

**The default is unchanged and still destroys in place.** For items that own nothing -- which is most of
them -- that is exactly right, and a queue of `u32` should not have to think about any of this. What
changed is that the behaviour now has a name and an alternative.

**The claim under test is about threads, not counts.** It would be easy to assert only that the sink
receives the items, which is the mechanism rather than the property. The suite instead records the
`ThreadId` a destructor runs on and asserts it is *not* the thread that released the last handle -- with a
control, without a sink, showing it *is*. That control matters: without it the first test would look
identical if destructors simply never ran anywhere observable.

Each shape walks its own layout to find survivors, so the routing is asserted once per shape rather than
once for the crate. That is the lesson M31.2's sweep taught about the reservation guarantee, applied
before the sweep had to teach it again.

## D-21: a panicking sink is caught, and the walk continues

The sink is caller-supplied code running inside a destructor, which is the worst place for it to panic.
A panic escaping there does one of two bad things: during an unwind it aborts the process, and otherwise
it abandons every item not yet disposed -- precisely the handles the mechanism exists to account for.

So the call is wrapped and the walk continues. This is deliberately **not** "swallowing an error": the
item has already been moved into the sink, so there is nothing left to report about it, and the item is
destroyed by the unwind rather than leaked. A sink that panics is a bug in the caller; catching only
declines to turn it into a much larger one.

`AssertUnwindSafe` is the honest annotation rather than a way past the bound. The only state observable
after a panic is the caller's own closure, and the queue's invariants do not depend on the sink at all --
teardown is already past the point where anything could observe them.

## D-22: no `into_remaining`, because it would not close the hole

The obvious API for shutdown is "consume the consumer, get everything that is left". It was considered
and refused, for two reasons that compound.

**It does not close the hole.** A consumer can take everything *available*, but producers may still push
afterwards -- so it covers the orderly path and nothing else, and the disorderly path is the one that
strands handles. The last handle to drop remains the only place that sees every survivor, which is where
[D-20](#d-20) puts the mechanism.

**And it adds nothing over what exists.** `Consumer::drain` already takes everything available; an
`into_remaining` would be that plus consuming the handle. Since the sink covers the case `drain` cannot,
the extra method would be surface without capability.

The orderly shutdown therefore stays what it already was: drain to empty, observe
`Consumer::is_disconnected`, and take the final item with the receive loop's `finish` step. The sink is
for everything that does not go to plan.

## D-23: high-water is opt-in; refusals and rings are not

R9 asks for three numbers, and the interesting thing about them is that they do not cost the same.

**A counter on a hot path is a shared line every thread writes** -- the same false-sharing cost the
positions are carefully padded apart to avoid. So each was placed where it is already paid for, and the
one that could not be placed that way became a switch:

| Metric | Where it increments | Cost |
|---|---|---|
| Refusals | only when a push is refused | off the success path entirely |
| Doorbell rings | only when `SetEvent` is actually called | ~7 ns against a syscall measured at ~81 ns |
| Peak depth | must observe **every** change | see below -- and it varies by shape |

Peak depth is the awkward one, and the awkwardness is not uniform:

- **`spsc`** -- free. The producer already loads `head` to decide there is room and owns `tail`, so the
  depth is a subtraction of two values in hand, and the counter's line is producer-owned.
- **`reserving_mpsc`** -- near-free, for the same reason: its producer reads `head` for the room check
  that honours reservations. Only the counter's line is shared, and it is written rarely.
- **`slotwise_mpsc`** -- *not* free. Its producer never reads `head`; that is the whole property
  [D-16](#d-16) built a separate shape to preserve, because `head` is the one line every thread touches.
  Tracking makes it read that line on every push.

Making it always-on would have imposed D-16's refused cost on every `slotwise_mpsc` user to serve a metric most of
them will never read -- and would have done it just before M31.5 measures
exactly that path. Omitting it from `slotwise_mpsc` would have narrowed the shape. So it is a switch, off by
default, and the cost lands only on queues that asked. Off, `slotwise_mpsc` pays one predictable branch on a field
written once at construction: the line is shared but read-only, which is the cheap kind.

**Untracked reports `None`, not `0`.** They are different answers -- "nobody was counting" versus "it
never filled" -- and a caller sizing a queue from the second when the first was true would be reading a
number nobody recorded.

**Two independent switches across three shapes is why `Options` is a builder.** As constructors that is
four functions per shape and twelve in the crate, and every future switch doubles it. The plain `bounded`
stays, because the default is the common case and should not have to say so.

`record_depth` loads before it modifies. An unconditional `fetch_max` would be a read-modify-write on a
shared line for every push; a new maximum is rare after a queue warms up, so the common case becomes a
plain load of a rarely-written line and the read-modify-write is reached only when the value is actually
about to change. The load may be stale, and the `fetch_max` behind it is what keeps the result correct
regardless -- which is asserted by a concurrent test rather than argued.

## D-24: counting the rings makes the skip part of the contract

R9 asks for the ring count so that "disabling the skip must change the number". Following that literally
has a consequence worth naming, because it inverts something this crate had already written down.

[`sabotage.json`](sabotage.json) carried an entry that removed the skip optimisation, expecting **`survives`**. It was a
*control*: skipping a redundant `SetEvent` changed no observable behaviour, so a suite that went red on
its removal would have been asserting the implementation instead of the contract -- and
[D-9](#d-9) records that the control earned its place by proving exactly that.

Once the rings are counted, that stops being true. The count is observable, so the skip is observable, and
the same patch that had to survive now has to be **caught**. The entry changed sides in M31.4.

**This is the requirement working, not a regression.** An optimisation nobody can measure is an
assumption, and R9's whole point is to stop this one being one. What it costs is that the skip is now part
of what the queue promises rather than a private cleverness -- so removing it later would be a behaviour
change, not a refactor. That is the right trade for a queue whose entire reason to exist is a wakeup
protocol, but it is a trade, and it should be made knowingly.

The control it vacated is replaced rather than dropped: `slotwise_mpsc`'s guard around the `head` load is an
optimisation and not a correctness device, so removing *that* must still leave the suite green. A sweep
with no controls left is a sweep that has stopped asking whether its tests describe the contract.

## D-25: `Observable` does not restate depth

[D-2](#d-2)'s sketch of this trait read "depth, high-water, doorbells actually rung". Depth was dropped on
the way to shipping it.

`Bounded::len` already reports depth, computed on demand from positions the queue keeps anyway. Naming it
again on `Observable` would give one number two spellings, two doc comments, and two places to drift --
which is the restatement problem this workspace has already paid for, recorded in the root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). The trait carries only what must be **accumulated**: facts about
the past that the queue's present state cannot reconstruct.

Both handles implement it, because both ends have a question. A producer wants to know how often it was
refused; a consumer wants to know how deep the backlog got and how often it was actually woken.

## D-26: the measurement, and D-16's premise falsified

Measured by `probe-queue-contention` in a **release** build on an AMD EPYC 7763, 8 cores / 16 logical
processors, Windows 11 Enterprise 10.0.26200, `x86_64`. Median of five repetitions after a discarded
warm-up; three independent invocations agreed to within noise. **Note the architecture**: every previous
measurement in this workspace was taken on the ARM64 development machine, so these numbers fill the x64
gap rather than extending the ARM64 record, and the two are not interchangeable.

Isolated regime -- producers only, capacity large enough that nothing is refused, so the curve is the
claim and nothing else:

| producers | `slotwise_mpsc` ns/push | `reserving_mpsc` ns/push | contended `fetch_add` |
|---|---|---|---|
| 1 | 9.0 | 8.6 | 5.0 |
| 2 | 49.0 | 28.0 | 8.1 |
| 4 | 84.4 | 33.3 | 12.2 |
| 8 | 140.8 | 38.5 | 13.7 |
| 16 | 193.5 | 52.2 | 14.5 |
| 32 | 239.7 | 56.9 | 15.1 |

**Two findings, and the second one was not the expected result.**

**The tail claim contends, and severely.** Aggregate throughput *falls* as producers are added: `slotwise_mpsc`
from 111M to 4.2M pushes per second, `reserving_mpsc` from 116M to 17.6M. A bare contended `fetch_add`
falls only to a third and then plateaus, so most of both curves is the queue rather than what this
processor does to a fought-over line.

**`reserving_mpsc` is up to 4x faster than `slotwise_mpsc` under contention**, which inverts [D-16](#d-16). That
decision shipped the two as peers on the reasoning that honouring a reservation costs the producer a read
of the consumer's position, making the reserving shape the expensive one. It is the cheaper one at every
producer count from two upward. The premise survives in exactly one place: a *single* producer against a
live consumer, where the drained regime measures 13.6 ns against 28.1 -- and at one producer the honest
answer is [`spsc`](crate::spsc) anyway.

The drained regime otherwise shows the two within 16% of each other at two, four and eight producers, and
its sixteen- and thirty-two-producer rows are consumer-bound -- millions of refusals -- so they measure the
single consumer rather than the claim.

## D-27: why, and why it is not a bug to fix

The obvious response to D-26 is that `slotwise_mpsc` must have a defect. It does not, and the difference is worth
understanding because it is a property of the two *protocols* rather than of two implementations of one.

Both do one compare-and-swap plus one load per attempt. The load is what differs:

- **`slotwise_mpsc` reads `slots[tail & mask].sequence`** -- and must, because in Vyukov's protocol the slot's own
  sequence is what says the slot is free. That address **marches through memory as the tail advances**,
  and the slots it walks are being written by the very producers it is racing.
- **`reserving_mpsc` reads `head`** -- one fixed address, which stays hot in every core's cache and, in
  the isolated regime, is never written at all.

So the reserving shape's extra read is cheaper than the read it *replaces*, which is why the measurement
came out backwards from the prediction.

**The false-sharing hypothesis was tested and rejected.** `Slot<u64>` is sixteen bytes, so four
consecutive positions share a cache line, and the obvious fix is to pad each slot onto its own. Measured:
at eight producers that moves `slotwise_mpsc` from 140.8 to 109.1 ns -- about a fifth -- for four times the
memory, and leaves it 2.8x slower than `reserving_mpsc`'s 38.5. False sharing between neighbouring slots
is a contributor, not the cause. The padding was reverted; the note on `Slot` that says slots deliberately
share lines is therefore correct, and now correct for a measured reason rather than an assumed one.

The remedy that *would* close the gap is to stop reading the slot before claiming and decide freedom from
`head` instead -- which is precisely `reserving_mpsc`'s protocol. There is no third design here to
discover: the two shapes are not "one queue with and without reservations", they are two different claim
protocols, and this measurement is the comparison between them.

**The merge-or-delete decision is therefore live and is the engineer's**, with the data above as its
basis. It is tracked as a checklist item rather than left here, because a decision recorded only in a
design note is not scheduled work.


## D-28: caching the peer's index was measured and rejected

**Amended -- the rejection held on x64 only, and ARM64 reverses it by 17x. The blanket "no shape adopts
it" no longer follows from the evidence; the open question is queued as
CHECKLIST-io-domains.md item M-inf.4.**

The engineer recalled a technique credited with taking queue throughput from millions to hundreds of
millions of operations per second: a load on the waiting side of the shared index. That memory is real
and it names a real optimisation -- **peer-index caching**, the standard trick in a high-performance
SPSC ring (Rigtorp). Each side keeps a plain, non-atomic copy of the *other* side's position. A
consumer whose cached `tail` says items are available drains them without touching the shared line at
all, and refreshes only when the cached copy says the ring is empty. One acquire load is amortised
over a whole batch, and the producer's release store stops invalidating a line the consumer reads
every iteration.

None of the three shapes did this. `spsc::push` acquire-loads `head` on every push, `spsc::pop`
acquire-loads `tail` on every pop, and `reserving_mpsc::push` acquire-loads `head` on every push.

**It was measured rather than adopted, and the measurement says do not adopt it.**
`probe-peer-index-cache` builds a minimal SPSC ring structurally identical to `spsc`'s and runs it
under three strategies -- baseline, peer-index caching, and a prefetch-only "warming" load kept as a
control -- while counting how many times each side actually reads the peer's position. Four release
runs on the x64 host agree:

| strategy | ns/item | consumer reads | producer reads |
|---|---|---|---|
| baseline | 18.7 - 27.7 | ~2.03 M | ~2.03 M |
| peer-index caching | 36.6 - 39.0 | ~0.56 M | ~2.2 - 2.6 M |
| warming load only | 20.0 - 24.0 | ~2.03 M | ~2.04 M |

The read counts are what make this conclusive, and they are the reason the probe counts them. **The
optimisation engaged**: consumer reads fell 3.6x. It engaged and still lost about 1.8x of throughput,
so this is not a failed implementation of the technique but a real result about our shape.

The mechanism is visible in the same columns. Peer-index caching trades *freshness* for fewer reads.
That trade is free when a genuine backlog exists, because a stale index is still far behind the peer
and the batch it amortises over is deep. Here the batch is only about 3.6 items deep -- a spinning
consumer keeps the ring near empty -- so each side repeatedly idles on a stale bound it could have
refreshed, and the idling costs more than the reads saved. On the producer side the count goes *up*:
a cached index is consulted only when it says "no room", so a producer that is genuinely blocked
refreshes on every spin iteration and gains nothing whatsoever.

The warming variant behaved exactly as a control should, which is what makes it worth having kept: it
removed no shared read (its counts match the baseline) and it moved no throughput. A discarded load
cannot help, because the authoritative load still happens and in a tight handoff loop the prefetch has
no time to land before it. **The engineer's "it is just for cache warming" reading is therefore not
the mechanism** -- the technique works by removing the load, not by warming the line for it.

### The deeper-batching workload was found, and it is simply the other architecture

The paragraph above says the trade "is free when a genuine backlog exists, because the batch it
amortises over is deep", and that here the batch is only ~3.6 items. **That mechanism is correct and it
is the reason the conclusion does not travel.** Re-running the identical release binary on the ARM64
development host (Snapdragon X2 Elite, 12 cores, no SMT, no L3), median of three:

| strategy | ns/item | consumer reads | producer reads | batch depth |
|---|---|---|---|---|
| baseline | 30.4 - 32.4 | ~2.1 M | ~2.1 M | ~1 |
| peer-index caching | **1.8** | ~9 - 19 K | ~3.4 - 3.6 K | **~150** |
| warming load only | 27.0 - 29.2 | ~2.0 M | ~2.1 M | ~1 |

**17x faster, not 1.8x slower**, and the producer read count falls by ~580x rather than rising. Every
observable this decision rested on inverted. What did not change is the *explanation*: batch depth
decides the outcome, and batch depth is a property of how the two threads interleave -- core count,
whether siblings share a core, how the scheduler places them -- not of our code. x64 kept them
lock-step; ARM64 lets them decouple.

**That last sentence was itself too coarse, and the x64 host disproved it.** See
"[the flip is placement, not architecture](#d-28-placement)" below: pinned to SMT siblings, the same
x64 machine that produced the rejection reverses it. The variable is placement, not instruction set.

Two consequences, and the second is the uncomfortable one:

- The blanket rule **"no shape adopts it"** does not follow from the evidence any more. It is now a
  choice between hosts, queued as CHECKLIST-io-domains.md M-inf.4,
  which asks for a *policy* for a technique whose sign depends on the machine rather than for more
  measurement. We have the measurement twice and it disagrees with itself.
- **The probe was printing this decision's conclusion as fixed prose.** It stated "the technique WORKED
  and still lost", "roughly 3.6x", and "on the producer side the count goes UP" unconditionally, so on
  ARM64 it contradicted its own table three lines above. Only the speedup ratio was computed. That is
  fixed -- the interpretation is now derived from the run, including the batch depths, and it says
  outright that the verdict has inverted by host. An instrument that reports its conclusion regardless
  of what it measured is worse than no instrument, because it is believed.

The two reasons below still stand, and neither is architecture-dependent:

- **The shared read is a minority of the cost.** The model runs at 18.7-27.7 ns/item while the
  shipping `spsc` runs at 58.6-62.8. This probe deliberately does not attribute that gap (the shipping
  push also consults the reservation count, updates the depth metric and rings the doorbell), but it
  does put a floor under the argument: whatever the shared read costs, removing it cannot be the large
  win.
- **It would be a correctness hazard at the arming boundary.** `Consumer::arm` decides whether to
  park, and that decision must be made against a fresh acquire load. A cached `tail` that says "empty"
  when the producer has already published is a lost wakeup -- the same defect class as
  [D-9](#d-9) and [D-15](#d-15), which this crate has now been bitten by twice.

The technique is not wrong; it is right for a ring with a standing backlog. **This crate's ring is that
ring on one of our two architectures and is not on the other**, which is the whole finding. The
re-measurement conditions written down here were "if a later workload shows deep batching" -- what
actually surfaced it was not a later workload but a second machine, and that is the more useful trigger
to remember. The arming path must be exempted from any cache regardless of the result, for the reason
below.

**Work is now scheduled by this decision**, where an earlier revision said none was. That sentence was
accurate when the answer was a flat rejection and is not accurate now: the open question is
CHECKLIST-io-domains.md M-inf.4. It is recorded so the technique is
neither re-proposed without measurement nor adopted on the strength of whichever host someone happened
to benchmark on.

### <a id="d-28-placement"></a>The flip is placement, not architecture -- and the sibling hypothesis was refuted backwards

The ARM64 host asked the x64 host to test a specific prediction: **that SMT siblings sharing L1 would
stay in lockstep, giving shallow batches, and that this was the condition making caching lose.** ARM64
has no SMT and physically cannot express that placement, so only the x64 host could answer it.

`probe-core-affinity`, x64, medians of three runs (all three agreed to within 3%):

| placement | base ns/item | cached ns/item | cached batch depth | verdict |
|---|---|---|---|---|
| SMT siblings (one core) | 10.7 | 5.9 - 6.0 | **116 - 163** | caching **WINS** 1.8x |
| same cache, same class | *not expressible* | | | |
| cross cache, same class | 19.3 - 21.8 | 38.7 - 42.1 | **1.7 - 1.8** | caching **LOSES** 2.0x |

**The hypothesis is refuted, and refuted backwards.** Siblings do not stay in lockstep -- they produce
by far the *deepest* batches measured on this host, and caching wins there. The shallow batches are on
the *cross-core* row, which is where caching loses. Sharing a cache causes decoupling, not lockstep.

This is the more important half of the finding: **the verdict flips inside a single machine.** The
earlier framing that "x64 keeps the threads lock-step and ARM64 lets them decouple" attributed to the
instruction set something that is a property of *placement*. The same x64 binary on the same x64 host
both wins and loses depending only on which two processors the threads land on. No decision keyed to
architecture can be correct, which is why the amended rule is placement-scoped.

It also explains the original rejection without contradicting it. Unpinned threads land on separate
cores, which is exactly the losing row; re-running `probe-peer-index-cache` unpinned reproduces it
(baseline 18.4 - 23.8 ns, cached 35.0 - 38.8 ns). The first measurement was never wrong -- it was one
placement reported as though it were the machine.

**Why the sign changes, unified across both hosts.** Caching wins when
`(cost of the shared read) x (reads saved)` exceeds the cost of idling on a stale bound. Both terms
move with placement:

- *Siblings* share L1, so the handoff is cheap (10.7 vs 19.3 ns even with no caching). The two threads
  interleave on one core's execution resources rather than running truly concurrently, so the producer
  bursts ahead and the consumer drains deep batches. Deep batch, caching wins.
- *Across cores* the ring ping-pongs one item at a time (depth ~1.7) and, on this host, the read being
  saved is cheap anyway -- crossing L2 while staying inside a shared L3, same package, same NUMA node,
  same efficiency class. Little saved, staleness paid. Caching loses.
- *ARM64 across domains* saves a genuinely expensive read (215 ns baseline), so it wins 3.5x even at a
  batch depth below 1. Cost per read, not just depth, is part of the trade.

**The x64 host isolates the cache effect, which ARM64 could not.** ARM64's cache domains and core
classes are confounded -- crossing one crosses the other -- so it cannot separate "cache domain cost"
from "core speed cost". This host has a single L3 domain, a single efficiency class, and eight L2
domains, so its `cross cache, same class` row varies *only* the cache domain, with class, package,
L3 and NUMA all held constant. That isolated crossing costs **1.8x - 2.0x** on the unoptimised
handoff. Conversely this host cannot express `same cache, same class` at all: its outermost
partitioning cache is L2, shared by exactly the two siblings of one core, so any two processors
sharing a cache domain are siblings. The two hosts are complementary rather than redundant, and
neither alone can produce the full table.

The two hosts are in fact **disjoint** -- no placement is measured by both -- so every row rests on a
single machine and none cross-checks another. The per-placement coverage matrix, including the one row
(`same cache, cross class`) that neither host can express, is kept with the open question in
CHECKLIST-io-domains.md M-inf.4 rather than duplicated here.

**A probe defect found while doing this, now fixed.** `probe-core-affinity` printed its placement
table from a hard-coded list of four variants that omitted `SameCoreSiblings`, while the
interpretation beneath it iterated over the placements actually measured. On an SMT host the table
therefore showed the sibling row as absent while the interpretation quoted a number for it -- the
single most important row on this machine, silently missing from the table that was supposed to
report it. This is the second time in this investigation that an instrument's *presentation* rather
than its measurement nearly produced a wrong conclusion (the first was the fixed-prose interpretation
noted above). The fix also makes the "near vs far" summary fall back to the sibling pair on hosts
where `same cache, same class` is not expressible, which would otherwise have printed nothing here.


## D-31: 0.1.0 ships without machine-checked orderings, and says so

Model-checker verification of the memory orderings (M31.6) does **not**
gate `windows-waitable-queues` 0.1.0. It gates 1.0. The gap is disclosed in the crate documentation and
the README rather than left for an adopter to discover.

The case for gating was strong and is worth stating before the case against. This is a lock-free
concurrency primitive whose whole value is correctness, and the suite's blindness to ordering defects is
**measured, not assumed**: weakening the producer's `Acquire` load of the consumer's position to
`Relaxed` left all twenty tests of the day green, while every logic defect injected beside it was
caught. Publishing with a known blind spot is a real decision.

Three things decided it the other way.

**A model checker would close the demonstrated gap but not the dangerous one.** It models atomics; it
cannot model `SetEvent` and `ResetEvent`. So it covers the queue shapes' positions and sequence numbers
-- which is where the weakened-acquire defect lives -- and it cannot cover the doorbell, whose entire
correctness argument ([D-9](#d-9), [D-15](#d-15)) is how an atomic mirror flag interleaves with those
two syscalls. Stubbing them verifies a model of `SetEvent` rather than `SetEvent`, which is the
"measures the model, not the thing" trap this workspace was already caught by once, in
[D-28](#d-28)'s probe. **The only ordering bug this crate has actually had was D-15's lost wakeup, it
was found by sabotage, and a model checker would not have found it.** Treating that work as "the
orderings are now verified" would therefore overstate it in exactly the direction that matters.

**The risk it addresses is mostly regression risk, and that is lowest now.** The orderings are believed
correct and were argued at the time; the sabotage sweep *introduced* the weakening to prove the suite
was blind to it, rather than discovering one. Regression risk grows with contributors, changes and
consumers, all of which begin after publication.

**Gating has a cost that is not paid by this crate.** It blocks 0.1.0, and through it the placement
tool and the measurements from other people's machines that the whole release sequence exists to
obtain -- see CHECKLIST-placement-tool.md. Every host available
here has one NUMA node; that is not fixable locally at any price.

**The disclosure is what makes this a decision rather than a deferral.** The crate says what is
verified, says that stress testing here is known not to catch ordering defects, cites the measurement
that shows it, and says a model checker is planned before 1.0. An adopter then decides with the
information we have. A `0.x` version number carries the rest, and is meant literally.

## D-29: both multi-producer shapes ship, and the caller is given the data instead of a verdict

[D-26](#d-26) falsified [D-16](#d-16)'s premise -- reading the consumer's position was supposed to make
`reserving_mpsc` the expensive shape, and it is instead the faster one under contention, by up to 4x on
x64 and 6.4x on ARM64. That reopened a question D-16 had treated as settled: if the split does not buy
what it claimed, should the shapes merge, or should one be deleted?

**Neither. Both ship, and the crate declines to choose between them on the caller's behalf.**

The two are not one queue with a feature flag. `slotwise_mpsc` implements Vyukov's bounded array protocol, where
a producer asks a slot's own sequence number whether it is free; `reserving_mpsc` counts free slots
against the consumer's position, which is the only way a reservation can be answered at all. Both are
independently studied designs with production track records, chosen by different systems for different
reasons. **Our own workload having settled which we want is a fact about our workload**, and treating it
as a fact about queueing would be exactly the narrowing PLATFORM INTEGRITY forbids: the absence of a
visible consumer for a design is not evidence that none exists.

What the crate owes a caller instead is honesty and equipment:

- **The measurements, stated plainly**, including the regimes where each wins and the fact that the
  answer inverted once already when a second architecture was tried.
- **The means to measure their own domain.** `probe-core-affinity` and the placement tool exist so a
  caller can settle this on their own hardware and workload rather than inheriting ours. A queue
  library that publishes one benchmark and calls it a recommendation is asserting a conclusion about
  machines it has never seen.

### What the split does *not* rest on

Two justifications are available and both are refused, because a rationale that evaporates on
inspection is worse than none:

- **Not capacity.** On a 64-bit target `slotwise_mpsc` reaches 2^62 slots and `reserving_mpsc` 2^31, and
  that difference is unreachable: it counts slots allocated at construction, not items ever pushed, and
  2^31 slots is tens of gigabytes before the ring holds anything useful. See [D-17](#d-17) for why the
  packing forces it. **On a 32-bit target the difference does not exist at all** -- the crate-wide
  ceiling is 2^30 and `reserving_mpsc`'s packed 2^31 is clamped down to it, so both shapes stop in the
  same place. Pinned by `the_shapes_ceilings_are_what_the_public_documentation_claims`, which is run
  against `i686-pc-windows-msvc` as well as the host.
- **Not `slotwise_mpsc` being faster somewhere.** Its one measured advantage is a single producer with a live
  consumer -- and at one producer the right shape is [`spsc`](#d-1), which is faster still and which
  this crate also ships. A shape kept for a regime already better served elsewhere is kept on
  sentiment.

The split rests on **capability**: `reserving_mpsc` implements `Reserving` and `slotwise_mpsc` cannot, for the
structural reason D-16's surviving half explains. Everything else is profile, and profile is the
caller's to measure.

### The obligation this creates

Keeping both doubles the surface that every later decision must cover, and that is accepted knowingly
rather than discovered later. `M31.6`'s loom verification covers both shapes or neither is verified;
`M-inf.4`'s peer-index policy is decided for both or the crate ships two different answers to one
question. **A shape kept for others' benefit is still a shape this crate maintains**, and the moment
that maintenance is skipped for one of them, the argument above stops being true.

## D-34: what the prior art actually protects, and why this crate is outside it

SH-14.1 is not a bug in an unusual design. It is the
standard design, used below the width at which the standard correctness argument holds. That
distinction is the whole content of this decision, and it took a survey to establish -- the full
record, with citations and with the gaps flagged, is in
[DESIGN-SESSION-2026-09-02](design-sessions/DESIGN-SESSION-2026-09-02-claim-protocol-prior-art.md).

### The protocol is mainstream

`crossbeam-queue::ArrayQueue`, `concurrent-queue` and `thingbuf` were each read at their push path.
All three load a counter, load a second value, decide from the pair, then compare-exchange **only
the counter** and write. None re-validates after the exchange. That is our protocol.

Searching all three for `ABA`, `wraparound`, `overflow` or any statement of a counter-width bound
returns nothing. The assumption is load-bearing in every one of them and written down in none.

### What actually makes them safe

Two mechanisms, and it is worth being blunt that neither is "the protocol is careful":

- **By width.** The counter is a whole `usize`, so returning to a given bit pattern needs on the
  order of 2^64 pushes. Nikolaev states this explicitly (DISC 2019, section 3, "ABA safety"): the
  counters "will not wrap around until after the number of operations exceeds **the CPU word's
  largest value**, a reasonable assumption made by other ABA-safe designs as well."
- **By structure.** CRQ and SCQ advance the shared counter with an unconditional fetch-and-add that
  authorizes nothing, and put the authorizing compare-exchange on the *cell*, where the reuse
  decision and the write live in one word. No observation survives the exchange unvalidated.

`reserving_mpsc` has neither. Its position is a 32-bit half of a packed word, not a machine word, so
the width argument does not reach it -- and its exchange covers the claim word but not the `head`
its room decision was computed from.

### The nearest published twin defends a different property

DPDK's `rte_ring` is our protocol almost exactly: 32-bit indexes, room computed against a separately
loaded counterpart, compare-exchange to claim. Its *Programmer's Guide* (6.5.4) justifies it as "we
can do subtractions between 2 index values in a modulo-32bit base: that's why the overflow of the
indexes is not a problem."

That defends **modular arithmetic of the difference**. It says nothing about a producer stalled
across a full recurrence, which is the hazard. Searching DPDK's ring library for "ABA" returns
nothing either. So the closest thing to a published defence of this design defends the wrong
property, and the gap is undocumented industry-wide rather than something we alone missed.

### The generalisation

Stated once, so it is not re-derived, and flagged as **ours**: no source phrases it this way, though
SCQ's cell compare-exchange and CRQ's double-width `CAS2` are both instances of it.

> The atomic operation that authorizes the write must cover everything the decision depended on.
> Where it does not, correctness rests entirely on the counter being too wide to recur.

This is the criterion any future claim protocol in this crate is judged against, and it is more
useful than the narrower "avoid ABA": it says *what to check*, and it explains why the same
structural window is harmless in crossbeam (word-width counter) and dangerous here (subfield).

### What this decision does not decide

Which fix to adopt. That is M15, which prototypes the central-permit claim and measures it rather
than arguing it -- necessary because [D-26](#d-26) already measured that the single shared line is
what collapses under contention, so a protocol that touches two shared lines instead of one is not
obviously cheaper. This decision records only the landscape and the criterion.

## D-35: the permit claim measured, and the result that inverts the expectation

Run by `probe-queue-contention` on the reference host (x86-64, 16 logical / 8 physical, SMT on),
release build, five repetitions per configuration with the median kept. The whole run was repeated
three times; the isolated numbers reproduced within noise except one outlier noted below.

### Isolated regime -- producers only, nothing ever refused

The cleanest measurement of the claim, because nothing else touches the queue. Nanoseconds per push:

| producers | `slotwise_mpsc` | `reserving_mpsc` | `permit_mpsc` | contended `fetch_add` |
|---|---|---|---|---|
| 1 | 6.3 | 5.5 | 8.0 | 2.4 |
| 2 | 58.6 | 33.9 | 42.8 | 12.7 |
| 4 | 89.6 | 33.5 | 31.5 | 14.7 |
| 8 | 143.4 | 37.9 | 26.1 | 15.9 |
| 16 | 225.1 | 56.1 | 20.5 | 14.6 |
| 32 | 234.3 | 53.1 | 19.5 | 13.4 |

`permit_mpsc` against `reserving_mpsc`, as a cost ratio: **1.45x, 1.26x, 0.94x, 0.69x, 0.37x,
0.37x**. The crossover is between two and four producers.

### The result, and why it was not expected

**The safer claim is also the faster one everywhere contention exists.** At sixteen and thirty-two
producers it is 2.7x cheaper per push than the shape it would replace, and 12x cheaper than
`slotwise_mpsc`.

That is the opposite of what the design predicted. `permit_mpsc` touches **two** shared lines on the
push path -- the permit count and the ticket -- where `reserving_mpsc` touches one plus a read of
`head`, and [D-26](#d-26) had already established that the single shared line is what collapses
under contention. The expectation was therefore that adding a second one would cost.

**The mechanism is retries, not lines.** Both of `permit_mpsc`'s operations are unconditional
read-modify-writes: `fetch_sub` on the permits and `fetch_add` on the ticket. Neither can fail, so
neither retries. `reserving_mpsc`'s claim is a `compare_exchange_weak` that retries once per lost
race, and at thirty-two producers almost every race is lost. The retry loop dominates the second
cache line long before the second cache line matters.

Two corroborating observations, both from the same table:

- **It is the only shape that gets *faster* per push as producers are added** -- 42.8 ns at two down
  to 19.5 at thirty-two. Every other shape, and the bare atomic floor, degrades monotonically. A
  claim that cannot fail has no retry storm to suffer, so added producers buy parallelism in the
  slot writes without adding claim work.
- **It is the only shape that stays close to the floor.** At thirty-two producers it costs 19.5 ns
  against a bare contended `fetch_add`'s 13.4 -- 1.46x, while doing two of them plus a slot write
  plus a doorbell ring. `reserving_mpsc` is 4.0x the floor there and `slotwise_mpsc` 17x.

### Where it loses, and why that is the honest reading

**At one producer it is 1.45x slower** (8.0 ns against 5.5). Uncontended, `reserving_mpsc` pays one
compare-exchange that always succeeds plus a load of an uncontended line -- and a load is far
cheaper than a read-modify-write. `permit_mpsc` pays two read-modify-writes regardless. The permit
claim converts a shared *read* into a shared *read-modify-write*, which is the wrong trade when
there is no contention and the right one when there is.

A single-producer queue is a real configuration, so this is a genuine cost and not a rounding error.
It is also exactly the regime in which [D-16](#d-16)'s surviving half already says `spsc` is the
right shape.

### What this does NOT decide

**The drained regime is not clean enough to read.** Its refusal counts differ between shapes by two
to five orders of magnitude and are unstable across runs -- `reserving_mpsc` recorded 0 refusals at
eight producers in one run and 2,363 in the next, and `permit_mpsc` recorded roughly 460,000 and
490,000. There are at least two candidate explanations, and this harness cannot separate them: the
permit shape is genuinely faster, so it attempts more pushes against a full queue; or its optimistic
overdraw refuses near-full more readily than the shipping shape's re-read does. Since the probe's own
caution is that a run with many refusals was waiting for the consumer rather than for the claim,
those rows price backpressure rather than admission.

The drained ratios are recorded for completeness -- 2.19x, 0.85x, 0.94x, 0.75x, 0.92x, 0.66x -- but
the isolated regime is the one that answers the question asked, and the refusal question is queued as
`SH-15.5.1` rather than resolved here.

**One outlier, recorded rather than dropped.** In the second of the three runs, `permit_mpsc` at
eight producers measured 56.7 ns against 26.1 and 26.8 in the other two, breaking an otherwise
monotone trend. Eight producers is exactly this host's physical core count, so scheduling variance
there is plausible; two of three runs agree closely and the trend either side of that point is
unambiguous. It is noted because a reader re-running this will likely see it too.

### What it means for SH-14.1

The ABA hole and the contention cost turn out to be **the same load**. `reserving_mpsc`'s read of the
consumer's `head` is simultaneously the stale input that SH-14.1 exploits and the shared access D-26
measured. Removing it for correctness removes it for performance as well, which is why this
measurement came out the way it did -- and why "closing the hole will cost throughput" was the wrong
thing to have worried about.

## D-38: one atomic, one discipline

An atomic that carries any acquire/release operation carries acquire/release on **every** operation.
A relaxed load is never mixed onto it.

The reason is not that relaxed is "weaker and therefore riskier". It is that a relaxed operation has no
memory ordering **at all**, so it is not placed at any defined point relative to the ordered operations
on the same object, and the code generator and the processor are both free to move it. With respect to
placement it behaves like a plain load or store: it is not pinned to its site in the source. Reading
such a program in textual order -- statement one, then statement two, then statement three -- and
concluding what the relaxed operation observes is not a weak argument; it is not an argument, because
the premise that it happens there is false.

**"Plain" above is about placement only, and the distinction is easy to lose.** A relaxed operation is
still fully atomic: indivisible, never torn, immune to the compiler inventing or duplicating accesses,
and coherent (all threads agree on a single modification order for that one location). What it gives up
is ordering with respect to *other* memory. The two axes are independent, and the confusion runs in both
directions -- "relaxed means no guarantees, so I may as well use a plain field" is as wrong as "relaxed
is ordered, just weakly", and it is the more dangerous of the two because the field it produces is a
data race. [D-40](#d-40) states the atomicity half, with this crate's claim word as the worked example.

What makes this expensive rather than merely wrong is that it **usually does what the author expected**:
a simple load or a simple store at the obvious place. It survives review, it survives testing, and it
breaks later, when an optimizer version changes or the code runs on a processor with a weaker model.

This crate is unusually exposed to that, for a reason already measured. On x86-64's TSO, a decorative
`Acquire` load and a `Relaxed` load compile to very nearly the same instruction, so **no test on this
host can distinguish them** -- which is precisely the blindness [D-31](#d-31) recorded when weakening a
real `Acquire` to `Relaxed` left all twenty tests of the day green. On AArch64 the same two loads are
`ldar` and `ldr`, and the difference is real. CI builds `aarch64-pc-windows-msvc`.

### The resolutions, and which one to take

For a mixed atomic there are two consistent repairs: **promote the loads to acquire**, or **demote the
stores to relaxed**. Both are valid, and choosing between them is a separate and more involved analysis
of what the atomic is actually for.

**The standing answer here is to promote the load.** An acquire that turns out to have been unnecessary
is a performance claim someone can come back and make with a benchmark. A relaxed load that turns out to
have been load-bearing is a defect that appears only on hardware we do not own. The asymmetry is not
close, so it is settled in advance rather than re-argued per site.

Demotion is correct only where the atomic has **no release operation anywhere** -- there being nothing to
pair with, an acquire load on it would read as a guarantee the type does not make. Three atomics are in
that position and are uniformly relaxed for that reason: `reserving_mpsc`'s claim word (every write is a
`Relaxed/Relaxed` compare-exchange), `permit_mpsc`'s `tail` and `head`, and `slotwise_mpsc`'s `tail`
(whose claim CAS is deliberately `Relaxed/Relaxed` and says so at the site).

Those four remain atomics, and the atomicity is load-bearing even with no ordering attached to it --
[D-40](#d-40). The claim word makes the point unmissable: it is a `u64` packing two `u32` halves, so a
torn read would produce a `(reserved, position)` pair that was never a state the queue was in. "Every
operation on it is relaxed" is therefore not a step toward making it a plain field; it is a statement
about ordering and nothing else.

### What the audit found

Every atomic in the crate was grouped by field and asked one question: does it have a release write, and
does it also have relaxed operations? Four atomics had **acquire loads with no release write anywhere**
-- the acquire pairing with nothing, synchronizing with nothing:

| Atomic | Acquire loads | Release writes |
|---|---|---|
| `reserving_mpsc` claim word | 4 | 0 |
| `permit_mpsc::tail` | 1 | 0 |
| `permit_mpsc::head` | 1 | 0 |
| `slotwise_mpsc::tail` | 1 | 0 |

Four more had a real release store with relaxed loads mixed in, and those loads were promoted:
`reserving_mpsc::head`, `slotwise_mpsc::head`, `spsc::head`, `spsc::tail`.

One had a genuine load-bearing edge with a relaxed operation sitting in the middle of it:
`permit_mpsc`'s permit counter, where the `Release` increment frees a slot and the `Acquire` decrement
claims it, but the overdraw undo was `Relaxed`. That one is rescuable by the release-sequence rule, but
that rule was narrowed once already (C++20 dropped same-thread relaxed stores from it) and it is not
something a reader should have to reconstruct to trust a slot handoff. It is now `Release`, which costs
one `stlxr` over `stxr` on AArch64, on the contended slow path.

Two categories were deliberately left alone. Reference counts are [D-39](#d-39). Reads through
`&mut self` in `Drop` are not atomic operations at all -- `get_mut` is a plain read, and there is no
second thread for an ordering to order against -- so `permit_mpsc`'s drop was converted to `get_mut`,
matching `reserving_mpsc`, which removes the question rather than answering it.

### Why the audit is a script and not a reading

The first two versions of the audit were both wrong, in opposite directions, and neither error was
visible without checking a result by hand. The first required the receiver on one line and so missed
every multi-line `self.shared\n.head\n.0\n.store(...)` chain, reporting three atomics as having no writes
at all. The second matched across newlines and started absorbing words out of comments, splitting one
atomic's operations into several phantom fields and inventing "decorative acquire" findings for atomics
whose release store it had filed elsewhere.

Both produced confident, plausible, wrong tables. The lesson is not about regular expressions: an
ordering audit's output has to be checked against the source at a few points before it is believed,
because a wrong audit here is indistinguishable from a right one by inspection.
