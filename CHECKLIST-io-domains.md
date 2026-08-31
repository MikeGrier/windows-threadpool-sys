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
  and worth publishing, unlike `windows-guard-alloc`), and write its `DESIGN-NOTES.md` with the decisions
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
  Crate created with `DESIGN-NOTES.md` (D-1..D-8), `README.md`, `PLANS.md` pointing back at this file, and
  registration in the workspace members, `release-please-config.json`, and `.release-please-manifest.json`
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
  **Done, and the signatures are written down in `spsc.rs`'s module documentation before the type**, as
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
  Delivered as `src/doorbell.rs`: lazily created (a poll-only consumer allocates no kernel object,
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

- [ ] **M31.1** -- The bounded array MPSC: Vyukov's sequence protocol, where a producer CASes the tail
  forward, writes, then publishes by storing the slot's sequence. Lock-free rather than wait-free, bounded
  by construction so backpressure is free, and no allocation anywhere. Pad the head and tail onto separate
  cache lines and say so in a comment, because the padding is load-bearing and looks like waste.

- [ ] **M31.2** -- Overflow policy, which is more than "return `Err`". Ship fail-fast plus a `reserve`
  that guarantees a slot for a message that must not be lost, following
  [queue.rs](crates/windows-file-watcher/src/queue.rs), which already carries three policies including a
  **coalesced loss latch** the consumer is guaranteed to observe. **Never offer overwrite-oldest**: for
  telemetry that is a lost sample, but for an I/O submission it is a lost operation, and the two must not
  share a policy knob.

- [ ] **M31.3** -- Shutdown in both directions: the consumer learns when every producer is gone, and a
  producer learns when the consumer is gone and fails with a typed error. Descriptors in flight at
  teardown are **accounted, not dropped** -- some own handles, and their disposal must be allowed to
  block, which is the hazard the namespace session flagged for undrained completions.

- [ ] **M31.4** -- Observability (R9): depth, high-water, and **a count of doorbells actually rung**. That
  last one is what makes the skip rule measurable rather than assumed, and sabotage-verifiable -- disabling
  the skip must move the number.

- [ ] **M31.5** -- The contention benchmark that decides whether the deferred shapes are needed: N producer
  threads pushing, throughput against N. **This is the item that either justifies or kills the linked and
  sharded MPSC shapes**, and it is deliberately a measurement rather than a judgement, for the same reason
  C-1 was. If the tail CAS does not contend at realistic producer counts, the array queue is the only MPSC
  this crate ever needs.

  Record the result either way -- a measurement that says "the simple thing is fine" is worth as much as
  one that does not, and is the cheaper outcome to lose track of.

- [ ] **M31.6** -- Verify the memory orderings with a model checker, because stress testing demonstrably
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
the runtime's shape, so all three land before M33+ begins.

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

> **-> CROSS-COMPONENT HANDOFF:** M33+ below spans `crates/windows-thread-ambient-sys`,
> `crates/windows-namespace-request-sys`, and `crates/windows-ioring-sys`. Each has its own
> `CHECKLIST.md`; the items are held here until M32 settles, then move to the component that owns them.

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

- [ ] **M-inf.2** -- The eventcount, if and only if a measurement against real I/O shows the doorbell
  costs enough to be worth its lost-wakeup risk. C-1 showed batching alone drives it below the atomic push
  it accompanies, so nothing currently justifies it.

- [ ] **M-inf.3** -- An allocation-model change to `PreparedPath` (inline storage or request recycling).
  Bounded before anyone builds it: `prepare` is dominated by `GetFullPathNameW`, a Win32 call no allocator
  removes, and cloning already-prepared units is 95 ns of a 453 ns request. That 95 ns is the ceiling on
  the win, and only for a caller that can reuse a resolved path.
