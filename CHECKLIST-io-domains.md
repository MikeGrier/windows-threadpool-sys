# Checklist: NUMA-sharded I/O execution domains

Feature-scoped checklist for the `mikegrier/deferred-namespace-ops` branch. It covers a new queue crate,
a domain runtime, a durability layer, and extensions to three existing crates, so it lives at the
workspace root -- their lowest common source-component -- rather than inside any one of them. Per the
naming convention for feature files, it is deleted outright once every item is complete, with the content
moved to [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

Authoritative decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md](design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md),
which is **still open**. Milestone numbers continue the workspace sequence: [CHECKLIST.md](CHECKLIST.md)
holds M19-M21 and [CHECKLIST-thread-ambient.md](CHECKLIST-thread-ambient.md) held M22-M27.

## What is ready, and what deliberately is not

**M30-M32 are specified.** The queue crate's requirements (R1-R10, as amended by the C-1 and
request-cost measurements) are settled, its shapes are chosen, and its boundary is decided. None of it
depends on NUMA hardware, and all of it is testable with no ring, no pool, and no I/O.

**M33+ is parked, not pending.** The domain runtime cannot be written until M32 settles three contract
questions -- ordering, correlation, and backpressure -- that would change its shape. Those are decision
items in M32 rather than assumptions baked into M33+, because the session recorded them as open and
promoting an open question to a settled one by writing code around it is exactly the failure this
sequencing avoids.

**The N=1 path is the whole of the first deliverable.** A single execution domain needs no routing
policy, no cross-domain queue, and no placement choice, so nothing below is blocked on the multi-node
hardware the session could not obtain. What N>1 adds is additive, not a second mode.

## M30 -- The queue crate: name, skeleton, and the SPSC shape

- [x] **M30.1** -- Decide the crate's name and record why, **before** anything depends on it, because
  renaming a crate that has dependents is churn this repository avoids. Two things to settle together:
  whether the `-sys` suffix applies (every existing `windows-*-sys` crate is thin-over-Win32, and this is
  a data structure with an opinion, so it probably does not), and the name of the domain runtime crate
  that will sit above it, since the pair should read as a pair. Candidates raised: `windows-io-queue`,
  `windows-signalled-queue`, `windows-queue`. Record the decision in [DESIGN-NOTES.md](DESIGN-NOTES.md)
  so the reasoning survives the choice.
  **Decided: `windows-waitable-queues`**, no `-sys` suffix, recorded in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#the-waitable-queues-crate-is-named-plural-and-carries-no-sys-suffix).
  The engineer proposed the plural and it is right for a reason stronger than taste:
  [windows-platform-probes](crates/windows-platform-probes/README.md) is already plural, so the workspace
  distinguishes singular-for-one-facility from plural-for-a-collection-of-peers, and this is the second
  kind. **`windows-io-queue`, floated during the same discussion, was rejected** -- the queues have
  nothing to do with I/O, and naming a general facility after its first consumer is the mistake
  `windows-topology-sys` avoided. What unifies them is waitability, a word this workspace already owns
  through `WaitableHandle`.
  **One consequence accepted deliberately:** the plural forbids a bare `Queue` type, since a crate named
  "queues" exporting one would claim a primacy the name denies. Every type is specifically named and a
  consumer must say which it wants.
  **The runtime crate's name is deliberately NOT fixed here**, which narrows this item as written. The
  churn argument applies to a crate with dependents, and M30.2 creates the queue crate immediately while
  the runtime does not exist until M33+. The rule is recorded instead -- the pair should read as a pair,
  and the runtime's name may carry `io` because that crate genuinely is about I/O.

- [x] **M30.2** -- Create the crate with `publish = true` (the engineer's decision: this is general-purpose
  and worth publishing, unlike `windows-guard-alloc`), and write its [DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md) with the decisions
  the session already reached: the shape menu and which shapes ship now, the
  concrete-types-plus-optional-trait rule, the overflow policy, and the doorbell invariant. This is the
  Tier-1 transcription of Tier-3 session content -- design notes are not a work queue, so a decision that
  lives only in the session record is orphaned.

  **Settle one question here that M30.3 then depends on:** whether `WaitableQueue` is a single
  consumer-side trait, or a producer trait and a consumer trait. Waitability lives on the consumer -- it
  waits, while the producer merely rings -- so a single trait would be consumer-side and the producer's
  contract would go unnamed. Decide rather than defer, because M30.3 writes the first signatures against
  whichever answer this gives.
  **Answered, and the question turned out to be the wrong one.** The engineer's observation that the
  shapes would be "sliced and diced by various traits as we go along" is right, and following it shows a
  single `WaitableQueue` trait -- one *or* a producer/consumer pair -- is not merely inelegant but
  **unimplementable by the shapes that are planned**: a poll-only queue has no doorbell to return, and an
  unbounded one has no capacity to report. So the answer is **narrow capability traits** on the
  `std::io` model (`Read`/`Write`/`Seek`, not one `Io`), each naming one capability, with a shape
  implementing the subset it genuinely has. Recorded as [D-2](crates/windows-waitable-queues/DESIGN-NOTES.md#d-2)
  with the anticipated set.
  **And no trait ships until a second implementation exists to validate it** ([D-3](crates/windows-waitable-queues/DESIGN-NOTES.md#d-3)):
  a trait written against one type designs in a vacuum, since every signature that type happens to have
  looks like a requirement. The trait *shape* is fixed now because it constrains M30.3; the traits
  themselves land with M31.1.
  Crate created with [DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md) (D-1..D-8),
  [README.md](crates/windows-waitable-queues/README.md), [PLANS.md](PLANS.md) pointing back at this file, and
  registration in the workspace members, [release-please-config.json](release-please-config.json), and
  [.release-please-manifest.json](.release-please-manifest.json)
  -- the last two because `publish = true` makes it release-managed, and omitting them would have left it
  silently unreleasable.
  **One earlier position reversed with its reason recorded** ([D-7](crates/windows-waitable-queues/DESIGN-NOTES.md#d-7)):
  shapes are plain modules, not Cargo features. Two features are four configurations against a
  `feature-matrix` CI job that would have to grow, and the benefit is one dead-code elimination already
  provides.

- [x] **M30.3** -- The SPSC bounded ring, with no doorbell and no Win32 at all: a pure data structure with
  acquire/release head and tail and no CAS on either side. It is the CQ direction (R1), and it is first
  because everything harder is a variation on it. Tests are ordinary fast unit tests -- capacity edges,
  wraparound, full and empty, and that a `pop` never observes a partially written `T`.

  **This item sets the shape every later queue must match**, so it is where the trait-compatibility
  constraint binds: split producer and consumer handles, with cardinality carried by whether each is
  `Clone` (see
  [DESIGN-NOTES.md](DESIGN-NOTES.md#the-waitable-queues-crate-is-named-plural-and-carries-no-sys-suffix)).
  Getting this wrong is not a local mistake -- if the first shape ships a signature the second cannot
  match, the `WaitableQueue` trait becomes a breaking change to one of them rather than an addition.
  Verify it the cheap way: write the trait's method signatures down as a comment before writing the
  type, and confirm the type satisfies them.
  **Done, and the signatures are written down in [spsc.rs](crates/windows-waitable-queues/src/spsc.rs)'s module documentation before the type**, as
  the item asked. `push`/`pop` take **`&self`**, not `&mut self`: the latter would also make
  single-producer sound and is what several SPSC crates use, but it cannot generalize to a shape where
  several threads push through a shared handle, and one spelling has to serve every shape.
  Cardinality is carried by the auto traits instead -- the handles are `Send` but **not `Sync`** and not
  `Clone`, so "single" is a fact the compiler checks. A multi-producer shape relaxes exactly one cell of
  that table.
  **Sabotage-verified rather than merely green.** Six deliberate defects, each confirmed to fail the
  suite: a drop loop starting at zero instead of `head`, an off-by-one in the full test, `Full` reported
  where `Disconnected` is owed, `pop` not advancing `head`, `push` not advancing `tail`, and a mask of
  `capacity` instead of `capacity - 1`.
  **One sabotage was NOT caught, and it is recorded rather than smoothed over:** weakening the producer's
  `Acquire` load to `Relaxed` leaves the suite green. That is a genuine limit of stress testing, not a
  missing test -- an ordering bug needs an interleaving the hardware and scheduler must be coaxed into
  producing, and neither ARM64 nor x86-64 will oblige on demand. Queued as M31.6.
  **A harness defect worth remembering:** the first sabotage sweep reported "not caught" for the
  `pop`-does-not-advance case, because the detector matched on the string `test result: FAILED` and the
  test process had instead died with `STATUS_HEAP_CORRUPTION`, which prints no such line. Nine tests had
  in fact failed. A sabotage harness that recognizes only one failure shape will eventually certify a
  hole that is not there -- or miss one that is. Detect by exit code.
  The same defect then bit the *gate*: piping `cargo clippy` through `Select-String` makes
  `$LASTEXITCODE` report the filter's status, not cargo's, so a clean-looking `exit=0` was hiding a real
  `-D warnings` failure (`clippy::doc_overindented_list_items`, seven sites in `windows-platform-probes`,
  actual exit 101) that CI would have caught. Fixed in the preceding commit. Redirect with `*>` and read
  `$LASTEXITCODE` before any pipe.

- [x] **M30.4** -- The doorbell, as its own reviewable unit: a queue-owned **manual-reset** event created
  **lazily**, so a polling-only consumer allocates no kernel object. Level semantics -- signalled exactly
  when the consumer has something to observe. **The reset must be atomic with the observation that there
  is nothing to take; the signal need not be** (C-1b measured why: a late signal is a spurious wakeup, a
  stale reset is a lost one). Hand it out as a borrowed handle plus an owned duplicate, per the
  file-watcher's precedent.
  **Landed together with M30.5 in one commit, because these two items are not independent and the
  checklist was wrong to split them.** A `Doorbell` that no queue calls is dead code, and this workspace
  builds with `-D warnings`, so M30.4 cannot compile on its own. Recorded as an acknowledged
  structuring defect rather than worked around by widening the type's visibility to silence the lint --
  making an API public to dodge a warning is a real design decision taken for a fake reason.
  Delivered as [src/doorbell.rs](crates/windows-waitable-queues/src/doorbell.rs): lazily created (a poll-only consumer allocates no kernel object,
  asserted, not assumed), manual-reset, with `handle` / `owned` / `signal` / `clear`. The redundant
  signal is skipped through an `AtomicBool` mirroring the event, which is sound in exactly one
  direction -- see the done-note on M30.5 for the asymmetry that permits it.

- [x] **M30.5** -- Join the two, and **sabotage-verify the lost-wakeup guard**: a test that reverses the
  reset and the emptiness check must deadlock, and must stop deadlocking when the order is restored. A
  wakeup invariant asserted only by a passing test is a test of nothing -- this is the same discipline
  the ioring crate's `wait_then_drain` and the M17.4 calibration established.
  **Done. The guard is `Consumer::arm`, which clears the doorbell and *then* checks emptiness** -- the
  reverse of the order that reads naturally, which is why it needed proving rather than asserting. The
  sabotage test drives the race deterministically on one thread (an interleaving that must be hit to
  prove a point is not one to leave to the scheduler) and ends in a real bounded `WaitForSingleObject`:
  reversed, it returns `WAIT_TIMEOUT` with an item sitting in the queue -- the lost wakeup, reproduced;
  correct, the check finds the item and never waits at all.
  **Corrected after review: the deterministic test named above tested a *copy* of the wrong order,
  not the real `arm`.** `arm_reversed_racing` is a hand-written duplicate with the two statements
  swapped, so it could only ever show that *a* reversed order is wrong -- it could not detect the real
  `arm` being reversed. Measured: sabotaging the real `arm` was caught in **one run out of three**,
  because detection then depended on two threads interleaving inside a window tens of nanoseconds wide.
  This is the anti-pattern CONTRACT INTEGRITY rule 1 names -- a second copy of a rule checks the copy,
  not the rule -- and it was found only because the sweep was re-run under a tighter bound and the
  result flipped. `Consumer::arm` now carries a `#[cfg(test)]` hook that fires between the clear and the
  check, so a test drives the **real** `arm` through that exact window on one thread. Caught every run
  since.
  **Thirteen sabotages, twelve defects and one control, all behaving as expected.** Caught: push not
  signalling; producer `Drop` not signalling; `arm` checking before clearing; `arm` not creating the
  doorbell before checking; the final drain returning nothing; `clear` resetting the event but not the
  mirror flag; auto-reset instead of manual-reset; the event created already signalled. Three of those
  are caught **as hangs rather than failures**, which is the correct shape for a lost-wakeup defect.
  **The control matters as much as the defects:** removing the skip-redundant-signal optimisation must
  *not* fail, and does not -- so the suite is asserting the contract rather than the implementation.
  **The sweep paid for itself twice, and neither finding came from reading the code.**
  (1) A test gap: the drain-after-disconnect guard sat in a race window no test could reach, so
  breaking it changed nothing. Fixed by extracting it as `Consumer::finish`, a named step a test can
  call directly instead of hoping to schedule the window.
  (2) A harness defect: one sabotage inserted `if false { signal(); }` beside the live call instead of
  deleting it, so it sabotaged nothing and the resulting pass read as a hole in the tests. A sabotage
  that does not sabotage is worse than none, because it retires a question that was never asked --
  always confirm the injected defect actually changes behaviour before believing a "not caught".

## M31 -- The MPSC shape and the queue's contract

- [x] **M31.1** -- The bounded array MPSC: Vyukov's sequence protocol, where a producer CASes the tail
  forward, writes, then publishes by storing the slot's sequence. Lock-free rather than wait-free, bounded
  by construction so backpressure is free, and no allocation anywhere. Pad the head and tail onto separate
  cache lines and say so in a comment, because the padding is load-bearing and looks like waste.
  **Done as `src/mpsc.rs`, since renamed to
  [slotwise_mpsc.rs](crates/windows-waitable-queues/src/slotwise_mpsc.rs)**, with the padding commented at *both* positions rather than once, since a
  reader arriving at either field is the one who might delete it. Recorded as
  [D-10](crates/windows-waitable-queues/DESIGN-NOTES.md#d-10).
  **The traits landed here too, because M30.2 scheduled them here** ("the traits themselves land with
  M31.1") and [D-3](crates/windows-waitable-queues/DESIGN-NOTES.md#d-3) required a second implementation
  to validate them against. **The signatures `spsc` wrote down in advance held unchanged**, which is that
  check actually being run rather than assumed, and the load-bearing one turned out to be `push(&self)`:
  `&mut self` would have been sound for one producer and would have made the trait *unimplementable* by
  this shape. Recorded as [D-11](crates/windows-waitable-queues/DESIGN-NOTES.md#d-11). `Reserving`,
  `LossReporting` and `Observable` are deliberately still absent -- they belong to M31.2 and M31.4, and
  shipping an empty trait now would be the design-in-a-vacuum D-3 forbids, one level up.
  **The protocol refused a capacity of one, and that is reported rather than worked around.** With a
  single slot, "published at `p`" and "free again at `p + capacity`" are the *same number*, so a producer
  would read the sequence of the item it had just pushed, conclude the slot was free, and overwrite an
  unread item. `spsc` accepts one, so the minimum is a property of the *shape*, not of the crate --
  `CapacityError` already carried a `max_valid` on exactly that argument and now carries a `min_valid`
  too. Every workaround considered puts a load of the consumer's position back on the producer's hot path
  for every queue, in order to serve a capacity of one that `spsc` already represents exactly.
  [D-12](crates/windows-waitable-queues/DESIGN-NOTES.md#d-12).
  **Two things were extracted rather than copied, and one of them is a contract.** The blocking receive
  loop *is* the arming protocol (D-9), not glue around it, so a second spelling of it would have been a
  second copy of a rule -- the exact mistake M30.5 already paid for, where a lost-wakeup proof exercised a
  hand-written duplicate of `arm` and could not have noticed the real `arm` being reversed. It now lives
  in [blocking.rs](crates/windows-waitable-queues/src/blocking.rs) with shapes binding to it, and the `ARM_RACE` hook is shared for the same reason
  ([D-13](crates/windows-waitable-queues/DESIGN-NOTES.md#d-13)). The capacity rule moved to [capacity.rs](crates/windows-waitable-queues/src/capacity.rs)
  on the weaker version of the same argument.
  **One question the checklist did not anticipate: what "empty" means for arming.** `len` and "would `pop`
  find something" disagree over a slot a producer has claimed but not published, and arming on `len` is
  safe but spins until that producer is rescheduled. Arming asks the readiness question instead, which is
  also what puts D-9's `SeqCst` pair on the right two locations for this shape
  ([D-14](crates/windows-waitable-queues/DESIGN-NOTES.md#d-14)).
  **This shape exposed a lost wakeup in the doorbell that `spsc` could not have found, and it was fixed
  at the layer that owns it.** `Doorbell::clear` cleared its mirror flag and *then* reset the event; a
  producer signalling between those two lines set the flag and issued a real `SetEvent`, and the
  `ResetEvent` that followed erased the signal while leaving the flag set -- so the doorbell was dark
  while claiming to be lit and every later signal skipped. The order had a written argument behind it
  ("the caller's re-check sees the racing producer's item") that is **true for `spsc` and false for
  `slotwise_mpsc`**, whose re-check asks only whether the *head* slot is published. The fix moves the guarantee
  from the caller to the type: once `clear` returns the flag is false, so no future shape has to have a
  re-check strong enough to cover the window. [D-15](crates/windows-waitable-queues/DESIGN-NOTES.md#d-15),
  which amends D-9 rather than being filed beside it.
  **It was found by the sabotage harness refusing to sweep against a red baseline** -- the baseline run,
  whose only job is to prove the suite is green before any defect is injected, hung once in a suite that
  passed 120 tests in 0.28s six runs running. A single unreproducible hang is the finding it is tempting
  to blame on a busy machine.
  122 unit tests and 4 doctests, the whole suite in 0.30s. Twenty-three sabotages, all behaving as
  declared: ten new ones for this milestone, two of them controls. **One of those controls earned its
  keep immediately** -- it caught the new doorbell test asserting the signal-skip *optimisation* rather
  than the contract, which is precisely what a control is for.

- [x] **M31.2** -- Overflow policy, which is more than "return `Err`". Ship fail-fast plus a `reserve`
  that guarantees a slot for a message that must not be lost, following
  [queue.rs](crates/windows-file-watcher/src/queue.rs), which already carries three policies including a
  **coalesced loss latch** the consumer is guaranteed to observe. **Never offer overwrite-oldest**: for
  telemetry that is a lost sample, but for an I/O submission it is a lost operation, and the two must not
  share a policy knob.
  **Done, and the multi-producer case forced a decision the item did not anticipate.** Honouring a
  reservation means knowing how many slots remain, which means reading the consumer's position -- one
  line every thread touches -- on *every* push, including the pushes that never reserve anything.
  `slotwise_mpsc`'s producer avoids that read by design: it asks the slot's own sequence "are you free", and those
  are dispersed across the slot array. So `slotwise_mpsc` genuinely cannot answer the reservation question, and
  rather than charge every caller for a capability not every caller wants, **`reserving_mpsc` ships as a
  peer and `slotwise_mpsc` is untouched** ([D-16](crates/windows-waitable-queues/DESIGN-NOTES.md#d-16)). The
  engineer chose this split over the alternatives when it was raised.
  **The reservation count and the claim position share one 64-bit word, and that is the correctness
  argument rather than tidiness** ([D-17](crates/windows-waitable-queues/DESIGN-NOTES.md#d-17)). With the
  count in its own atomic, a pushing producer reads it then writes the position while a reserving one
  writes it then reads the position, and each can miss the other -- granting a slot that does not exist.
  **`SeqCst` fences do not close this**, unlike the superficially identical hazard in D-9: the Dekker
  argument needs store-then-load on both sides and the pusher is load-then-store, so both sides missing
  each other is consistent with every total order. The 32/32 split is forced by the arithmetic and caps
  the shape at 2^31 items, reported through the same per-shape bound D-12 introduced for the minimum.
  Two consequences worth noting: redeeming is a single exchange that moves both halves, so
  `occupied + reserved` is never momentarily wrong; and the producer stops needing the slot sequence for
  the "free" direction, so this shape's `pop` is one store *shorter* than `slotwise_mpsc`'s.
  **A 128-bit compare-and-swap was raised, refused, and then adopted for a separate shape.** D-18
  refused it; **[D-37](crates/windows-waitable-queues/DESIGN-NOTES.md#d-37) supersedes that** -- read
  D-37 rather than this paragraph, which has already restated a superseded version once. In short:
  widening *this* shape's word would make its contract depend on the target, because
  `i686-pc-windows-msvc` has no lock-free 128-bit exchange and the fallback is a silent global lock.
  So the narrow shape keeps its 64-bit word on every target, and a wide claim ships as its own peer
  where the exchange is genuinely lock-free.
  **`spsc` reserves too, nearly free**, since one producer means `reserve` and `push` are the same
  thread. Its reservation *borrows* the producer where `reserving_mpsc`'s is owned and `Send`, because
  there the handle **is** the single-producer guarantee and an owned reservation could outlive it on
  another thread. That difference is why `Reserving`'s associated type is generic over a lifetime, and it
  is D-3 working: the trait was shaped by two implementations rather than around one.
  **The loss latch is deliberately not generalised, and the reason is recorded rather than skipped**
  ([D-19](crates/windows-waitable-queues/DESIGN-NOTES.md#d-19)). Coalescing works in the file watcher
  because a desync is *idempotent*; a queue of arbitrary `T` has no such property. What generalises is a
  loss *count*, which is M31.4's observability rather than an overflow policy.
  **Two defects surfaced, both caught rather than reasoned about.** `Reservation::send` used
  `mem::forget` to suppress its double-release, which leaks the `Arc` the reservation holds -- so the
  shared state was never dropped and every item still in the ring leaked with it; found by the
  drop-counting test. And the first `const` assertions guarding the packing were **tautological**,
  asserting that `BOUNDS_MAX` equalled its own definition; widening the position to 40 bits sailed past
  them while silently narrowing the count's field to 24, which is how the packing actually breaks. Both
  rewritten, and the assertions verified by sabotage -- a too-wide and a too-narrow split each now fail
  the build with the right message.
  **And the sweep found a third, which is the one it exists to find: `spsc`'s reservation guarantee was
  entirely untested.** Every reservation test had been written against `reserving_mpsc`, and because the
  two implementations share *nothing* -- a plain counter against a packed compare-and-swap word --
  covering one left the other completely unguarded. A green suite cannot show that; only asking "would
  these tests fail if the code were wrong" can. Writing the missing wakeup test then surfaced something
  worth keeping: **the compiler refuses to move an `spsc` reservation to another thread at all**, because
  it borrows a `!Sync` producer. That is the borrow doing its job, so it is now a `compile_fail` doctest
  rather than a sentence -- itself verified by removing the attribute and confirming the error is the
  `Send`/`Sync` one and not a typo.
  167 unit tests, 5 doctests and 1 `compile_fail` doctest, the whole suite in 0.31s. Thirty-one
  sabotages, all behaving as declared: eight new ones for this milestone.

- [x] **M31.3** -- Shutdown in both directions: the consumer learns when every producer is gone, and a
  producer learns when the consumer is gone and fails with a typed error. Descriptors in flight at
  teardown are **accounted, not dropped** -- some own handles, and their disposal must be allowed to
  block, which is the hazard the namespace session flagged for undrained completions.
  **The first two clauses were already shipped, and this was audited rather than assumed.** Every shape
  has `is_disconnected` on both ends, `PushError::Disconnected(T)` hands the item back to a producer whose
  consumer is gone, and M31.2 added `Disconnected<T>` for a reservation redeemed into a dead queue. A
  reservation also counts as a producer, so an outstanding promise holds the stream open. Nothing was
  needed there; saying so is the point, since the alternative is checking a box on work done elsewhere.
  **The third clause was the whole item, and the default was quietly bad.** Undrained items were destroyed
  *in place*, inside the last `Arc` release -- so `T`'s destructor ran on whichever thread happened to drop
  last. That thread is not knowable in advance and nobody chose it: it may be a pool callback that must not
  block, and the namespace session's example is closing a handle to a dead network path, which is exactly
  the blocking operation the facility exists to keep off a caller's thread.
  **`Drop` cannot be made to hand them back** -- `&mut self`, no return, cannot fail, and by then every
  handle is gone so there is nobody to return them *to*. That is why `Disposal` is supplied at
  construction rather than requested at teardown: the last handle to drop is the only place that sees
  every survivor, and it is the one place with no way to report
  ([D-20](crates/windows-waitable-queues/DESIGN-NOTES.md#d-20)). The default is unchanged and still
  destroys in place, because for items that own nothing that is exactly right -- what changed is that it
  now has a name and an alternative.
  **The claim under test is about threads, not counts.** Asserting only that the sink receives the items
  would test the mechanism rather than the property, so the suite records the `ThreadId` a destructor runs
  on and asserts it is *not* the thread that released the last handle -- with a control, without a sink,
  showing that it is. Without that control the first test would look identical if destructors simply never
  ran anywhere observable.
  **Two smaller decisions recorded rather than left implicit.** A panicking sink is caught and the walk
  continues ([D-21](crates/windows-waitable-queues/DESIGN-NOTES.md#d-21)), because a panic escaping a
  destructor abandons every item behind it and aborts outright during an unwind. And `into_remaining` was
  considered and refused ([D-22](crates/windows-waitable-queues/DESIGN-NOTES.md#d-22)): producers may push
  after the consumer is consumed, so it would cover only the orderly path, and `drain` already does what
  it would do.
  Routing is asserted once per shape rather than once for the crate, since each walks its own layout --
  M31.2's sweep taught that lesson about the reservation guarantee, and this applies it before the sweep
  had to teach it twice.
  194 unit tests, 7 doctests and 1 `compile_fail` doctest, the whole suite in 0.33s. Thirty-six sabotages,
  all behaving as declared: five new ones for this milestone.

- [x] **M31.4** -- Observability (R9): depth, high-water, and **a count of doorbells actually rung**. That
  last one is what makes the skip rule measurable rather than assumed, and sabotage-verifiable -- disabling
  the skip must move the number.
  **The interesting thing about the three numbers is that they do not cost the same**, and each was placed
  where it is already paid for. Refusals increment only on the failure path. Rings increment only when
  `SetEvent` is actually called -- ~7 ns against a syscall measured at ~81 ns -- and the *skipped* signals
  are deliberately not counted, because that increment would land on exactly the path the skip exists to
  cheapen. Depth needed nothing new at all: `Bounded::len` already computes it from positions the queue
  keeps anyway.
  **High-water is the one that cannot be placed that way, and the cost is uneven in a way that lands on
  D-16.** A peak must observe every change. On `spsc` that is free (the producer already reads `head` and
  owns `tail`) and on `reserving_mpsc` near-free (its producer reads `head` for the room check), but
  `slotwise_mpsc`'s producer **never reads `head`** -- that is the property D-16 built a separate shape to
  preserve. Always-on would have imposed D-16's refused cost on every `slotwise_mpsc` user, to serve a metric most
  will never read, immediately before M31.5 measures that exact path. Omitting it would have narrowed the
  shape. So it is **opt-in at construction**, off by default, and `slotwise_mpsc` pays one predictable branch on a
  read-only field when it is off ([D-23](crates/windows-waitable-queues/DESIGN-NOTES.md#d-23)). The
  engineer chose this over the narrow-trait and always-on alternatives when it was raised.
  Untracked reports `None` rather than `0`, because "nobody was counting" and "it never filled" are
  different answers and only one of them should make a caller shrink a queue.
  **Two independent switches across three shapes is why `Options` is now a builder**, replacing M31.3's
  `bounded_with_disposal`. As constructors that is four per shape and twelve in the crate, with every
  future switch doubling it. The crate is unreleased, so the replacement cost nothing.
  **One consequence is worth naming because it inverts something already written down**
  ([D-24](crates/windows-waitable-queues/DESIGN-NOTES.md#d-24)). [sabotage.json](crates/windows-waitable-queues/sabotage.json) carried a *control* that
  removed the skip optimisation expecting `survives` -- and it had earned its place, by proving the suite
  asserted the contract rather than the implementation. Counting the rings makes the skip observable, so
  the same patch now has to be **caught**, and the entry changed sides. That is R9 working rather than a
  regression: an optimisation nobody can measure is an assumption. What it costs is that the skip is now
  part of what the queue promises, which is the right trade for a queue whose reason to exist is a wakeup
  protocol -- but it is a trade. The vacated control is replaced rather than dropped, by `slotwise_mpsc`'s
  tracking guard, which is genuinely an optimisation and must still survive removal.
  **`Observable` deliberately does not restate depth**
  ([D-25](crates/windows-waitable-queues/DESIGN-NOTES.md#d-25)), though D-2's sketch listed it: `len`
  already reports it, and one number with two spellings is two places to drift.
  221 unit tests, 9 doctests and 1 `compile_fail` doctest, the whole suite in 0.30s. Thirty-nine
  sabotages: three new, one converted from control to defect, and one new control replacing it.

- [x] **M31.5** -- The contention benchmark that decides whether the deferred shapes are needed: N producer
  threads pushing, throughput against N. **This is the item that either justifies or kills the linked and
  sharded MPSC shapes**, and it is deliberately a measurement rather than a judgement, for the same reason
  C-1 was. If the tail CAS does not contend at realistic producer counts, the array queue is the only MPSC
  this crate ever needs.

  Record the result either way -- a measurement that says "the simple thing is fine" is worth as much as
  one that does not, and is the cheaper outcome to lose track of.

  **Also measure `reserving_mpsc` against `slotwise_mpsc`, and decide their merge-or-delete here.** M31.2 shipped
  them as two shapes because reservation costs the producer a read of the consumer's position on every
  push, and *how much* that costs was a judgement rather than a measurement
  ([D-16](crates/windows-waitable-queues/DESIGN-NOTES.md#d-16)). This benchmark already stands up N
  producers against a tail, so measuring both under the same harness is nearly free.
  The decision it forces: if the shared-line read turns out to be cheap at realistic contention, the two
  shapes **merge** and the non-reserving one goes; if it is expensive, both stay and the split is
  vindicated. This item exists because a duplicated path silently becoming permanent -- because nobody
  circled back -- is the failure mode the duplicate-then-decide rule actually warns about, and an
  intention recorded only in a design note is not scheduled work.
  **Done, and both answers were surprises.** The probe is
  [queue_contention.rs](crates/windows-platform-probes/src/queue_contention.rs), run by hand in release on
  an AMD EPYC 7763 (8C/16T, x64), median of five with a discarded warm-up; three invocations agreed.
  **Note the architecture -- every previous measurement in this workspace was ARM64**, so this fills the
  x64 gap rather than extending the record, and M31.7 exists to close the other half.
  **The tail claim contends, so the licence to close M-inf.1 was not granted** -- but the gate there is
  now a number rather than a judgement, because contending and being the bottleneck are different things.
  See M-inf.1 for the quantified trigger.
  **`reserving_mpsc` is up to 4x FASTER than `slotwise_mpsc` under contention, which inverts D-16's premise**
  ([D-26](crates/windows-waitable-queues/DESIGN-NOTES.md#d-26)). The split shipped on the reasoning that
  reading the consumer's position made the reserving shape the expensive one; it is the cheaper one at
  every producer count from two upward, and the premise survives only at a single producer against a live
  consumer -- where `spsc` is the right answer anyway.
  **Investigated before concluding, at the engineer's direction, and the gap is intrinsic rather than a
  fixable flaw** ([D-27](crates/windows-waitable-queues/DESIGN-NOTES.md#d-27)). Both protocols do one CAS
  plus one load per attempt; the difference is *which* load. `slotwise_mpsc` must read the slot's own sequence
  before claiming -- an address that marches through memory as the tail advances, written by the producers
  it is racing -- where `reserving_mpsc` reads one fixed `head`. The false-sharing hypothesis was tested
  and rejected: padding each slot onto its own cache line recovers about a fifth at eight producers for
  four times the memory, and leaves the shape 2.8x slower. The padding was reverted.
  **A methodological trap worth keeping: a debug build reports the two shapes as identical** (249.7 vs
  254.0 ns at sixteen producers, against 193.5 vs 52.2 in release). That is why this probe is deliberately
  *not* in the CI probe job, which runs debug -- it would produce a confident wrong answer rather than a
  noisy one.
  **The merge-or-delete decision is now live with data behind it and is queued as M31.8**, not taken
  here: the investigation changed what the decision is *about*, from "is the extra read cheap" to "which
  claim protocol should survive", and that is the engineer's call.

- [x] **M31.7** -- Re-run `probe-queue-contention` on the ARM64 development machine and record the curve
  beside the x64 one. **Not a formality.** M31.5's finding is a statement about cache-coherence
  behaviour, and this workspace has already been bitten once by measuring only on ARM64 --
  [windows-platform-probes](crates/windows-platform-probes/DESIGN-NOTES.md) records that case. M31.5
  inverted a design premise on x64 evidence alone; if ARM64 disagrees, the merge decision in M31.8 changes
  with it, and so does M-inf.1's threshold.
  Run it in **release**: a debug build reports the two shapes as identical, which is why the probe is not
  in CI.
  **Done. Both of M31.5's claims hold on ARM64, and the reserving advantage is larger, not smaller.**
  Host: Snapdragon X2 Elite (Qualcomm Oryon), 12 cores, no SMT, no L3, two L2 clusters of six. Release
  build, median of three runs of the binary, isolated regime, ns/push:

  | producers | mpsc | reserving | atomic floor | mpsc/reserving | x64 ratio for comparison |
  |---|---|---|---|---|---|
  | 1 | 6.5 | 6.1 | 2.7 | 1.1x | 1.0x |
  | 2 | 29.8 | 9.4 | 3.6 | 3.2x | 1.8x |
  | 4 | 60.6 | 12.9 | 5.2 | 4.7x | 2.5x |
  | 8 | 167.4 | 29.8 | 8.8 | 5.6x | 3.7x |
  | 16 | 194.9 | 30.6 | 10.6 | 6.4x | 3.7x |
  | 32 | 195.0 | 30.6 | 9.9 | 6.4x | 4.2x |

  Claim 1 (throughput falls as producers are added) holds: `slotwise_mpsc` costs 30x more per push at 32
  producers than at one. Claim 2 (`reserving_mpsc` is up to 4x faster) holds and is exceeded -- **6.4x
  here against 4.2x on x64**. So M31.8's merge decision is not weakened by the second architecture; the
  evidence for the head-based protocol is stronger on ARM64 than it was on x64.
  Two differences worth having on the record rather than smoothing away. `slotwise_mpsc` **plateaus at ~195 ns
  from 16 producers upward** where x64 kept climbing to 239.7 -- expected, since this host has 12 cores
  and no SMT, so 16 and 32 are oversubscribed and the curve saturates. And **N=4 is by far the noisiest
  point** (`slotwise_mpsc` ranged 49.5 to 104.1 across the three runs, against under 2% spread at N=16 and above);
  with two six-core L2 clusters and no L3, whether four threads land inside one cluster or straddle both
  changes the answer, and at N>=8 straddling is forced so the variance disappears. Read the N=4 row as a
  range, not a point.

  > **-> CROSS-COMPONENT NOTE:** this run also contradicted D-28, which is recorded against that decision
  > and against M31.8's use of it below, not here.

- [x] **M31.8** -- **MIRRORED BY [CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md)
  SH-1.1 -- one piece of work seen from two plans. Check both off in the same commit; neither is done
  alone.** That file also records why this is *release*-blocking rather than merely design-blocking:
  the decision may delete a public type, which is free before `windows-waitable-queues` 0.1.0 and a
  yank-and-migrate after it.
  Decide merge-or-delete for `slotwise_mpsc` and `reserving_mpsc`, now that M31.5 has measured
  them and M31.7 will have checked the other architecture.
  **The decision changed shape once the investigation ran.** M31.2 framed it as "if the shared-line read
  is cheap, the two merge and the non-reserving one goes". The read is not merely cheap -- it is cheaper
  than the read it replaces -- so the real question is **which claim protocol survives**: Vyukov's
  sequence, which reads a marching slot, or the head-based one, which reads a fixed line.
  The candidates, with what each costs:
  - **Delete `slotwise_mpsc`, keep `reserving_mpsc`.** Simplest surface, and the faster shape under contention.
    Loses the 2x advantage `slotwise_mpsc` holds at one producer with a live consumer, and lowers the maximum
    capacity from 2^63 to 2^31 for every caller.
  - **Keep both**, and correct their documentation, which currently states D-16's falsified premise as
    the reason the split exists. The split would then be justified by *profile* -- one shape for few
    producers, one for many -- which is a real distinction but a harder one to explain.
  - **Change `slotwise_mpsc`'s protocol** to decide freedom from `head`, closing the gap. This makes the two
    shapes genuinely "one queue with and without reservations", which is what D-16 assumed they already
    were, and is the only option that removes the surprise rather than documenting it.
  Whichever is chosen, D-16's and `slotwise_mpsc`'s own documentation must be corrected in the same change: they
  currently assert a cost relationship the measurement reversed. That sweep is part of this item.
  **An input that was written off has come back, and this paragraph previously said the opposite.**
  Peer-index caching is available to the head-based protocol and structurally unavailable to Vyukov's.
  This item used to record that `probe-peer-index-cache` had measured it as making our ring *slower*
  (D-28), and instructed that it "must not be argued as" a differentiator. **That instruction was based
  on x64 evidence alone, and ARM64 reverses it**: the same binary measures caching at **17x faster**
  there (31.2 -> 1.8 ns/item), with the mechanism D-28 itself names -- batch depth -- coming out at ~150
  items per shared read instead of the ~3.6 that made it lose on x64. See D-28, now amended.
  So this **is** live as a differentiator, and it points the same way M31.7's contention curve does: it
  is an optimisation only the head-based protocol can adopt, and on one of our two architectures it is
  worth an order of magnitude. Do not resolve M31.8 by reinstating the old "it does not matter" line.
  What it is *not* is settled. The technique wins on one host and loses on the other, so adopting it
  unconditionally is as unsupported as rejecting it was. The decision this item owes is about the
  protocol; whether any shape then *adopts* caching is a separate question that needs a policy for a
  measurement that inverts by host, and that question is M-inf.4 rather than this item.

- [ ] **M31.6** -- **GOVERNED BY [CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md)
  SH-1.2, which decides only whether this blocks the 0.1.0 release. SH-1.2 completing does NOT complete
  this item**; it records an answer here.
  **The answer, recorded 2026-08-31: this does NOT gate `windows-waitable-queues` 0.1.0. It gates
  1.0**, and the crate ships 0.1.0 disclosing the gap in its own documentation rather than leaving an
  adopter to find it. See D-31. This item stays open, and the disclosure is a promise it now carries.

  **Its scope is corrected by the same decision, and this is the part worth reading before starting.**
  Loom models atomics; it cannot model `SetEvent`/`ResetEvent`. So it covers the three queue shapes'
  head/tail/sequence orderings -- which *is* where the demonstrated blind spot lives -- and it does
  **not** cover the doorbell, whose correctness is precisely the interleaving of an `AtomicBool` mirror
  with those syscalls. Stubbing them would verify a model of `SetEvent` rather than `SetEvent`, which
  is the "measures the model, not the thing" trap this workspace has already been caught by once.
  **D-15's lost wakeup, the only ordering bug this crate has actually had, was found by sabotage and
  loom would not have found it.** Do not let completing this item be read as "the orderings are now
  verified": the doorbell needs a separate answer, and this item does not supply it.
  Verify the memory orderings with a model checker, because stress testing demonstrably
  cannot. **Measured, not assumed:** during M30.3's sabotage sweep, weakening the producer's `Acquire`
  load of `head` to `Relaxed` left all twenty tests green, while every *logic* defect injected alongside
  it was caught. A stress test can only observe the interleavings the hardware and scheduler happen to
  produce, and neither ARM64 nor x86-64 will produce the reordering that makes a missing acquire visible
  just because a test asks nicely.
  `loom` is the tool: it enumerates interleavings under a weak-memory model rather than sampling them, so
  a missing `Acquire`/`Release` pair becomes a deterministic failure. It is a dev-dependency and a
  `cfg(loom)` shim over the atomics, so it costs the shipped crate nothing.
  **Sabotage-verify the verifier**, exactly as here: the loom test is only worth its weight if
  reintroducing that same `Relaxed` makes it fail. If it does not, the model is not covering the path
  and the test is decoration.
  Scope it to the orderings, not the logic -- loom explores exponentially, so a loom test that also
  checks FIFO order over a thousand items will not terminate.
  **A second, sharper target arrived from the M30.4/M30.5 code review, and it is the more important
  one.** The doorbell carries two `SeqCst` fences -- before the loads in `Doorbell::signal`, after the
  stores in `Doorbell::clear` -- which defeat a store-buffer (Dekker) reordering between the producer's
  decision to skip signalling and the consumer's emptiness check. Without them the item is queued, no
  signal is raised, and the consumer parks forever. **Removing either fence leaves the entire suite
  green**, and no sabotage can express it, because the defect is a fact about the memory model rather
  than an interleaving a scheduler can be coaxed into producing. Both fences must therefore be loom's
  first two subjects, and the test earns its place only if deleting each one makes it fail.
  Note that loom must model the doorbell's `OnceLock` publication too, since the lazy-creation path is
  one of the two sides of the hazard; a loom test that only models the steady state will pass with the
  `signal` fence removed and prove nothing about the case that motivated it.

## M32 -- Contracts the runtime cannot be written without

These are decision items, not implementation. Each is open in the session record, and each would change
the runtime's shape, so they land before M33+ begins. (The heading said "all three" while listing four;
it now lists five, and the count is dropped rather than maintained.)

- [ ] **M32.1** -- **The ordering guarantee.** Open since the 2026-08-27 namespace session, which
  observed that `DeleteFile(X)` then `CreateFile(X)` on a pool does not execute in order and said the
  contract "must state this explicitly rather than let it fall out of the implementation". A
  single-consumer SQ gives per-domain FIFO *for free* -- the question is whether it is **promised**.
  Promising it constrains every future implementation; withholding it makes composition harder for a
  client that has an ordering requirement and no other way to express one. Decide, and state the
  guarantee in the queue's own documentation rather than leaving it as an artifact.

- [ ] **M32.2** -- **Correlation.** Who mints the tag that joins a submission to its completion, and how
  it survives the two-layer translation into the ring's own `user_data`. Constraints already established:
  `IoRing` mints `user_data` starting at **0** on a fresh ring, and `Token::claim_if` requires both
  `user_data` **and** `RingId` to match. The client-facing tag is therefore not the ring's tag, and the
  mapping between them is state the domain owns.

- [ ] **M32.3** -- **Backpressure behaviour.** R2 says a full queue fails, and that failure is the
  backpressure. But a client with nowhere to go either spins or drops, so decide whether a blocking submit
  exists -- and if it does, **what it blocks on**, because a blocking submit that cannot be composed into
  a `WaitForMultipleObjects` reintroduces exactly the wait-composition problem that ruled out crossbeam.

- [ ] **M32.4** -- Transcribe the session's converged decisions from Tier 3 into Tier 1, and record the
  ones this checklist rests on in [DESIGN-NOTES.md](DESIGN-NOTES.md): the uniform tunable architecture,
  report-don't-route, the domain runtime not being a thread pool, the rejection of round-robin, and the
  two-layer ring. **A decision recorded only in a session record steers nothing**, and this checklist is
  the mechanism that makes them binding.

- [ ] **M32.5** -- **Note that the shard plan is *not* one of these contracts, and where it went.**
  M33+.1 opens with "one pinned thread, its `IoRing`, its node-local registered pool, its shard",
  which presupposes a mapping naming which thread, which node and which shard -- and that mapping was
  unowned: M32's other four contracts are all about the queue, and no item anywhere computed the plan.
  It is now [crates/windows-execution-plan](crates/windows-execution-plan/COMPONENT.md), a component
  of its own, because it applies **policy** over the topology's facts and reasonable clients will
  choose differently. Nothing here needs to decide it; this item exists so a reader of M33+ does not
  conclude the mapping is obvious, which is how it went missing.

> **-> CROSS-COMPONENT HANDOFF:** M33+ below spans `crates/windows-thread-ambient-sys`,
> `crates/windows-namespace-request-sys`, and `crates/windows-ioring-sys`. Each has its own
> [CHECKLIST.md](CHECKLIST.md); the items are held here until M32 settles, then move to the component that owns them.
>
> **The plan M33+ executes comes from
> [crates/windows-execution-plan/CHECKLIST.md](crates/windows-execution-plan/CHECKLIST.md)**, which is
> itself gated on the locality-model design session. So M33+ has two prerequisites, not one.

## M33+ -- The domain runtime (gated on M32)

Parked, not pending. Shape recorded so it is not lost, per the `M{n}+` convention.

- [ ] **M33+.1** -- The domain: one pinned thread, its `IoRing`, its node-local registered pool, its shard.
  N=1 first and complete on its own; N>1 adds routing and a cross-domain queue without disturbing it.

- [ ] **M33+.2** -- The thread builder, into
  [windows-thread-ambient-sys](crates/windows-thread-ambient-sys/README.md): construct a thread with
  `PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY` set **at creation**, because a stack is allocated then and
  binding afterwards cannot move it. Plus `bind_current_thread` with a restore guard for threads the
  client did not create. **Its principal justification is unverified** -- see
  [thread-stack-numa-spike.rs](crates/windows-ioring-sys/design-sessions/spikes/thread-stack-numa-spike.rs),
  which is written and smoke-tested but needs multi-node hardware. If it comes back showing creation-time
  affinity does *not* govern stack placement, this item shrinks to the binder alone.

- [ ] **M33+.3** -- Extend [windows-namespace-request-sys](crates/windows-namespace-request-sys/README.md)
  so an `Outcome` can carry the volume-node hint and its provenance alongside the handle, which is the
  "report, don't route" primitive.

- [ ] **M33+.4** -- The `threadpool` feature: a client-side helper for multiplexing more CQ doorbells than
  `WaitForMultipleObjects` accepts, using `ThreadpoolWait` (kernel-side wait completion packets, so wide
  waits cost the dispatch hop rather than a thread per 64). **Default-off and at the edge** -- a domain
  waits on three handles and never approaches the limit, so the dependency belongs to whoever multiplexes.

- [ ] **M33+.5** -- The durability layer as its own crate: **composition with shared vocabulary, not
  derivation.** It contains a domain and submits through it; it re-exports `Op` and `Completion` where the
  concept is genuinely the same, and defines `Epoch` and its own commit types where it adds meaning.
  Carry one constraint from the start: the flush barrier stops at the ring's edge, so **an epoch is
  per-domain** and a client spanning two domains needs two flushes and an explicit join.

## M-inf -- Ungated

- [ ] **M-inf.1** -- The linked and sharded MPSC shapes, if and only if M31.5 shows the array queue's tail
  CAS contends at realistic producer counts.
  **M31.5 has run, and the gate is now quantified rather than open.** The tail claim *does* contend, on
  x64: aggregate throughput falls with every producer added, `slotwise_mpsc` from 111M to 4.2M pushes/sec and
  `reserving_mpsc` from 116M to 17.6M, against a bare contended atomic that falls only to a third. So the
  licence M31.5 offered to close this item outright -- "if the tail CAS does not contend, the array queue
  is the only MPSC this crate ever needs" -- was **not** granted.
  **But contending is not the same as being the bottleneck, and this stays parked on that distinction.**
  At eight producers `reserving_mpsc` still sustains ~26M pushes/sec, or ~39 ns per push. A sharded queue
  is worth building only for a consumer whose per-item work is *smaller* than the contention it would
  remove, and the I/O domain this crate was written for is nowhere near that: C-1 already established
  that a real request dwarfs the queue's mechanics.
  So the trigger is now a number rather than a judgement: **build these when a consumer appears whose
  per-item cost is on the order of the ~39 ns/push (8 producers) or ~57 ns/push (32 producers) that the
  array queue's claim costs under contention.** Until then a sharded queue would optimise the small half.
  Re-measuring on ARM64 is the cheap way to find out whether that threshold moves; see M31.7.

- [ ] **M-inf.2** -- The eventcount, if and only if a measurement against real I/O shows the doorbell
  costs enough to be worth its lost-wakeup risk. C-1 showed batching alone drives it below the atomic push
  it accompanies, so nothing currently justifies it.

- [ ] **M-inf.3** -- An allocation-model change to `PreparedPath` (inline storage or request recycling).
  Bounded before anyone builds it: `prepare` is dominated by `GetFullPathNameW`, a Win32 call no allocator
  removes, and cloning already-prepared units is 95 ns of a 453 ns request. That 95 ns is the ceiling on
  the win, and only for a caller that can reuse a resolved path.

- [ ] **M-inf.5** -- **Domain-local queue placement**, now that the cost of getting it wrong is measured.
  `probe-core-affinity` finds an SPSC handoff costs **38.5 ns/item within a domain and 215.3 ns/item
  across domains on the ARM64 host -- 5.6x for nothing but where the two threads run**. That is far
  larger than any micro-optimisation this crate has considered, and it is a *placement* decision rather
  than a code one, which puts it squarely in the runtime's remit rather than the queue's.
  **This item's premise -- that a domain is a set of interchangeable processors -- is itself untested,
  and is now queued.** Every measurement behind the 5.6x pins to a *single* processor (`mask = 1 <<
  cpu`), so "place the thread in the domain" has only ever been evaluated as "place the thread on one
  chosen member of the domain". A set mask permits placements a single-processor mask forbids,
  including both ends of a queue on one logical processor, which on an SMT host is the *common* case
  for a same-cache set rather than a corner one.
  **FED BY [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) M6.7, which does not check this
  item off** -- this item is the domain-local placement work itself -- **but whose answer must be
  written in here when it lands.** M6 measures set-wide against pinned affinity under two interference
  models and reports migration and co-residency counts, so a null result can be distinguished from a
  scheduler that never moved anything. **If the sets are not equivalent,
  the 5.6x is a number about pinned threads and this item needs restating**, not merely re-measuring.

  The design already intends one pinned thread per domain, so the queue between two threads of the same
  domain is the common case and is fine. What this measurement bounds is the **cross-domain** queue --
  the one M30's design deferred on the grounds that N=1 does not need it -- and the number to carry into
  that decision is 5.6x, not zero.
  Gated on the domain runtime existing (M33+.1), not on more measurement.
  **Do not read the 5.6x as a cache effect or as a core-speed effect.** On the ARM64 machine the
  efficiency classes and cache domains coincide exactly, so the two are perfectly confounded.
  **We now do have a host that separates them.** The x64 host has one L3 domain and one efficiency
  class across eight L2 domains, so its `cross cache, same class` row varies the cache domain alone,
  with class, package, L3 and NUMA held constant: the isolated cache-crossing cost there is
  **1.8x - 2.0x** on the unoptimised handoff. That is not the same 5.6x boundary -- it is a shallower
  crossing (L2 inside a shared L3, not a cluster-to-cluster hop) -- so it bounds rather than
  decomposes the ARM64 number. Do not subtract one from the other.

- [ ] **M-inf.4** -- Peer-index caching in the head-based shapes, and more importantly **a policy for an
  optimisation whose sign depends on the host.** Gated on that policy, not on more measurement -- we
  already have the measurement, twice, and it disagrees with itself.
  D-28 rejected the technique on x64, where the producer and consumer stayed lock-step at a batch depth
  near 1 and caching cost ~1.8x. M31.7 re-ran the same binary on ARM64 and got a batch depth around 150
  and a **17x speedup**. Both are real; the variable is how the two threads interleave, which is a
  property of the host (core count, SMT, cluster layout, scheduler placement) rather than of our code.
  So the question this item owes is not "is it faster" but **what do we ship when a technique is a large
  win on one supported machine and a loss on another.** The candidates, none of them free:
  - **Ship it off**, as today. Costs ARM64 an order of magnitude on a shape that could have it.
  - **Ship it on.** Costs x64 roughly 1.8x on the same shape.
  - **Adapt at run time** from an observed batch depth, which is the only option that could win on both
    and is also the only one that puts a heuristic in the push path -- and a mispredicting heuristic is
    worse than either fixed choice.
  - **Make it a construction-time option**, pushing the decision to a caller who may know their
    producer/consumer coupling better than we do, at the cost of a knob nobody can set well without
    running the probe themselves.
  Whichever is chosen, it must be stated as a *policy* the crate owns rather than as a fact about a
  processor -- see PLATFORM INTEGRITY: this is exactly a lower baseline that must not be quietly dropped
  because the machine on the desk today prefers the other answer.
  **Placement was tested on both hosts, and the picture is now complete.** `probe-core-affinity` was
  written to check whether the host difference was really a *placement* difference. On ARM64 alone it
  looked eliminated: caching wins at **both** placements there (14.4x within a domain, 3.0x across), and
  threads placed together batch ~135x *deeper* than threads placed apart. Running it on x64 changed the
  answer -- **the verdict flips inside that single machine**: pinned to SMT siblings, caching WINS 1.8x
  at a batch depth of 116-163; pinned across cores it LOSES 2.0x at a depth of 1.7. Unpinned threads
  land across cores, which is precisely the losing row, so D-28's original result was one placement
  reported as though it were the machine.
  The unified rule both hosts obey: caching wins when `(cost of the shared read) x (reads saved)`
  exceeds the cost of idling on a stale bound. **Both terms are placement-dependent, which is why one
  term alone never explained it.** ARM64 wins even at depth ~0.4 because the read it saves is genuinely
  expensive (215 ns baseline); x64 loses at a similar depth because its cross-core read is cheap
  (19-21 ns -- crossing L2 while staying inside one L3, one package, one NUMA node, one efficiency
  class). So the sign is predictable from the two terms, and this item's policy question is unchanged
  but now better posed: **the knob is placement, not architecture**, and any policy keyed to the
  instruction set would be keyed to the wrong variable.
  **What the x64 host contributed that ARM64 could not.** ARM64's cache domains and efficiency classes
  are perfectly confounded (see M-inf.3's caution at the top of this section). The x64 host has one L3
  domain, one efficiency class, and eight L2 domains, so its `cross cache, same class` row varies only
  the cache domain -- the isolated cache-crossing cost is **1.8x - 2.0x**. It cannot express
  `same cache, same class` at all, because its outermost partitioning cache is L2 and is shared by
  exactly the two siblings of one core. The two hosts are complementary; neither alone produces the
  full table, and M-inf.3's "we do not have such a host" caution should be read against that.

  **Coverage, stated explicitly, because the two hosts turn out to be disjoint rather than
  overlapping.** Not one placement is measured by both machines, so no row is a cross-check of another
  and every row rests on a single host:

  | placement | ARM64 (Snapdragon X2) | x64 (EPYC 7763 slice) | measured by |
  |---|---|---|---|
  | SMT siblings (one core) | not expressible (no SMT) | **yes** | x64 only |
  | same cache, same class | **yes** | not expressible (L2 is per-core-pair) | ARM64 only |
  | same cache, cross class | not expressible (confounded) | not expressible (one class) | **neither** |
  | cross cache, same class | not expressible (confounded) | **yes** | x64 only |
  | cross cache, cross class | **yes** | not expressible (one class) | ARM64 only |
  | cross NUMA node | not expressible (one node) | not expressible (one node) | **neither** |

  A machine cannot express a placement when its topology makes the pair impossible: ARM64 confounds
  cache domain with efficiency class (crossing one crosses the other), and the x64 slice has exactly
  one efficiency class and one L3 with L2 shared only by SMT siblings.

  `same cache, cross class` -- two *different* cores sharing a cache domain but differing in class --
  **is unmeasurable on either host, and no re-run of either will produce it.** It needs heterogeneous
  cores inside one cache domain.

  **`cross NUMA node` is the other unmeasured row, and the probe was silently unable to report it
  until now.** `ProcessorPlace` carried core, class and cache domain but *not* the NUMA node, and
  `Placement` had no node dimension, so a cross-node pair would have been bucketed under a cache label
  with nothing in the output saying so -- and `representative_pairs` picks whichever pair it enumerates
  first, so which one you got would not have been reproducible either. On a scarce NUMA machine that
  would have produced a large number attributed to the wrong cause. Fixed ahead of the machine rather
  than after it: `numa_node` is now carried, `Placement::CrossNumaNode` is classified *first* (crossing
  a node dominates cache and class, exactly as sharing a core does), and six tests cover it including
  the precedence cases. Verified by sabotage -- removing the check fails four of them.
  This is the same defect class as the omitted `SMT siblings` row, and the third time in this
  investigation that an instrument's *classification or presentation*, rather than its measurement,
  was the thing about to produce a wrong answer.

  **The node-crossing path is validated against synthetic multi-socket topologies, offline.**
  `classify` and `representative_pairs` are pure functions of a processor list, so a mocked list
  exercises them exactly as real hardware would; five fixtures cover a two-socket host with several
  cache domains per node, one with a single cache domain per node, a no-SMT server, and a four-node
  host. Each asserts not merely that the expected rows appear but that **the pair chosen for each row
  actually satisfies that row's predicate** -- a table with right labels and wrong pairs behind them
  is worse than a missing row. All nine node-related tests fail when the classifier's node check is
  removed. The *timings* are deliberately not mocked and cannot be: `measure` pins to real
  processors, and pinning to one that does not exist fails loudly rather than fabricating a number.

  **Inter-node distance is measured per hop, not collapsed into one row.** `CrossNumaNode` is a single
  placement however many nodes exist, so on a host with three or more it would report one hop and
  imply the rest were like it -- and which hop you got would depend on enumeration order. Real
  multi-node hardware is not equidistant: two nodes on one package are far closer than two across a
  socket link. `node_pairs` therefore selects one representative processor pair per *distinct* node
  pair, and `measure` reports each hop separately in `by_node_pair`.
  **Corrected 2026-09-02:** this said the selection was "keyed `(low, high)` so a link is measured
  once rather than once per direction", which the code has not done for some time -- it keys
  `(producer.numa_node, consumer.numa_node)` and its comment states that "both *directions* are
  kept", with `by_node_pair` adding that "each hop is measured once per ring placement, so there are
  two". Four measurements per undirected edge, not one. Found while stating
  [EP-D-3](crates/windows-execution-plan/DESIGN-NOTES.md#ep-d-3), whose whole subject is that
  residency is directional, so a parked item asserting the opposite would have been read as evidence
  against it. The probe prints the resulting table, names the
  cheapest and dearest hop, and says outright whether the spread is small enough for the single
  `cross NUMA node` row to be a fair summary.
  **These are measured hops, not a firmware distance matrix.** Windows exposes no NUMA distance table
  -- there is no Win32 equivalent of reading ACPI SLIT -- so measuring the handoff is the only way to
  learn that two nodes are further apart than another two. Seven tests cover the selection on
  synthetic 1-, 2-, 3-, 4- and 8-node hosts, including that the hop count is the triangular number of
  the node count, that selection is stable across calls, and that non-zero-based node ids still work;
  all five relevant ones fail when the canonical-ordering guard is broken. On the single-node hosts we
  have, the section prints nothing rather than an empty table.

  **A prediction that will otherwise look like a bug on the real run: on a multi-socket host,
  `cross cache, same class` may be absent entirely.** `cache_domain` is defined as the outermost cache
  level that *partitions the machine*, so its meaning moves with the host. On the single-socket EPYC
  slice that level is L2, and the row measures an L2 crossing inside one L3. On a two-socket box whose
  last-level cache is per-socket, that level becomes the socket -- so two cores either share the cache
  domain (same node) or sit on different nodes, the node check claims the pair first, and the
  cross-cache row has no members. A synthetic fixture pins this down. **Read that absence as the
  topology speaking, not as a defect**, and note the corollary: the EPYC slice's isolated 1.8x - 2.0x
  cache-crossing number may have no counterpart on a multi-socket host at all.

  **A third host is planned -- an Intel cloud dev box -- and it should be expected to add no new rows.**
  An earlier revision of this item predicted it would express four placements at once, on the
  assumption of a *hybrid client* part (P-cores with SMT, E-cores without). That assumption is wrong
  for a cloud VM: cloud Intel means Xeon, which has no efficiency cores, so `ec[...]` will almost
  certainly read as a single class exactly like the EPYC slice, and the two class-crossing rows stay
  inexpressible.
  **All three hosts are VM slices, and a slice flattens topology.** The EPYC slice is the proof
  already in hand: a 7763 is 64 cores across eight CCXs each with its own L3, and 16 of those cores
  would span two of them -- yet `probe-topology` reports `L3[16]`, a single domain, and a single NUMA
  node. The hypervisor presented a flat view. **So the missing rows are not merely unmeasured, they
  are probably unreachable from any dev-box-sized VM slice**, and expecting a third slice to supply
  them would repeat the error of expecting a third architecture to.
  The Intel slice is still worth running, for a narrower and more honest reason: **it tests whether
  the SMT-sibling result reproduces on Intel Hyper-Threading rather than AMD SMT.** That row currently
  rests on one machine and one vendor's implementation of the feature, and it is the row carrying the
  claim that sharing L1 produces deep batches. A second SMT vendor either strengthens it or breaks it.
  Run `probe-topology` first regardless: whether the outermost partitioning cache is L2 or L3 decides
  which rows exist at all, and on a VM slice it is not predictable from the part number.
  **What would actually add rows**, if either becomes available: bare metal for `same cache, same
  class` and `same cache, cross class`, or a deliberately large multi-NUMA VM SKU (not a dev box) for
  a genuine node crossing. See the NUMA gap recorded below before spending time on the latter.

  **A two-socket Sapphire Rapids host is expected to become available, and it fills nearly the whole
  table at once.** Sockets give a genuine node crossing and a real `cross cache, same class` row at the
  L3 level; SMT gives the sibling row; two cores within a socket give `same cache, same class`. If
  **Sub-NUMA Clustering** is enabled it subdivides each socket, so the machine may present four or
  eight nodes -- and that would be the first host on which the node-pair matrix shows *variation*
  rather than one hop, because an intra-socket SNC hop and a cross-socket hop are not the same
  distance. That matrix was built for exactly this case and has never met a machine that can populate
  it.
  Three things to carry into that run:
  - **It still cannot produce `same cache, cross class`.** The cores are homogeneous, so that row
    stays unmeasurable on every host we have access to.
  - **It is x86-64, so it is TSO.** It will expose weakened memory orderings no better than the EPYC
    slice did, and a clean run there must not be read as ordering validation. ARM64 remains the more
    revealing host for that, and per D-31 neither substitutes for a model checker.
  - **It will present multiple processor groups**, which the tooling does not yet handle and would
    silently collapse rather than refuse. That is
    [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) M1B, and it must land before the machine
    is used.
  **Ask for `probe-topology` output first**, before any long run. Whether SNC is on, how many groups
  the host presents, and where its partitioning cache sits decide what everything else means, and none
  of the three is knowable from the part number.
