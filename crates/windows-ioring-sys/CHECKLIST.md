# Checklist: windows-ioring-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-22-ioring-architecture.md](design-sessions/DESIGN-SESSION-2026-08-22-ioring-architecture.md).
Everything through M18 is archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md): M1-M6
[here](COMPLETED-CHECKLIST.md#moved-2026-08-22----m1-through-m6-ring-lifecycle-through-consumer-documentation),
M7 [here](COMPLETED-CHECKLIST.md#moved-2026-08-23----m7-ring-copy-a-topology-aligned-sample), M11-M14 in their
own dated groups, M8-M10
[here](COMPLETED-CHECKLIST.md#moved-2026-08-30----m8-through-m10-handle-lifetime-cross-ring-identity-and-the-contract-audit),
and M15-M18
[here](COMPLETED-CHECKLIST.md#moved-2026-08-30----m15-through-m18-the-testing-strategy-response-to-eight-defects).

**`M20` is pending; `M6+` is parked rather than pending** -- see the `M{n}+` convention: it is gated work
with no current obligation, not an unfinished milestone. `M19` below is complete and awaits archival with the
next group.

## M19 -- Close the `get()` borrow hole (was release-blocking for 0.2.0)

Found by the code review of the M15-M18 branch, and **measured**: safe code can hold the `&[u8]` that
`RegisteredBuffers::get` returns across a submit that makes the kernel write into that same buffer. A probe
observed the bytes change from `0x11` to `0xEE` through the live borrow while a *fresh* `get(0)` at that same
instant correctly refused with `WouldBlock`.

The [D-36](DESIGN-NOTES.md#d-36) fix checks `kernel_writes` at the instant of the call, but returns a slice
whose lifetime is tied to `&self` -- and `Batch::read_registered` takes the registration by **shared**
reference, so the borrow and the read coexist. `get_mut` is unaffected: `&mut self` conflicts with the shared
borrow, so the compiler already rejects the analogous sequence. Only `get` is exposed.

This matters more than its severity alone suggests: it is the same hazard class D-36 was filed to close, in
the API whose breaking change 0.2.0 is being cut for, and it is reachable with no `unsafe` anywhere.

- [x] **M19.1** -- Make the borrow's *existence* conflict with starting a read into that buffer, not merely
  its creation. The options differ in what they cost, and the choice is the engineer's:
  (a) `get(&mut self)`, which is one line and closes it completely, but forfeits the concession D-36
  deliberately kept -- a caller could no longer read a buffer while its own *write* is in flight, which is
  sound because a write means the kernel only reads;
  (b) a `with_bytes(i, |bytes: &[u8]| ...)` callback, or a returned guard type holding a reader count that
  `begin_use(KernelAccess::WritesBuffer)` then refuses against, which preserves (a)'s concession at the cost
  of a wider API change.
  **Chose (a), after the engineer's question collapsed the choice:** does a caller ever *need* access during
  the hazard window? No -- while a read is in flight the bytes are indeterminate, partially written in
  arbitrary order, and only become meaningful once the completion is observed; while a write is in flight the
  caller wrote them and already knows. Earlier or later is always available, so the concession (b) preserves
  has no legitimate use.
  **The codebase agreed before the change was made:** all ~40 read sites across tests, examples and the
  epoch-log sample already read at a quiescent point -- their own `expect` messages say "is quiet", "is quiet
  again", "neighbour slot is quiet". Converting them needed nothing but `mut` on ten locals; not one held a
  borrow across a submit.
  The SAFETY comment now states an invariant the signature actually provides, and the rustdoc explains why
  `&mut self` hands back a shared slice.
  **The arena pattern is intact**, which was the thing worth checking: a `Token` holds a `RegisteredUse`, not
  a borrow of the registration, so quiet neighbours stay readable while operations are outstanding.

- [x] **M19.2** -- Add the regression test the probe became: hold the borrow across a submit and assert the
  bytes cannot change, or that the sequence no longer compiles. Verify it by sabotage like every other
  instrument in M15-M18 -- reverting the fix must turn it red.
  **Done as a `compile_fail` doctest on `get`**, since the hazard is now a type error rather than a runtime
  one. **Sabotage-verified:** reverting the signature to `&self` makes it fail -- the sequence compiles again,
  which is exactly the hole.
  Paired with a `no_run` doctest asserting the *neighbour* case still compiles, because a `compile_fail`
  passes on any error and a guard that over-constrained the arena would look identical to one that did not.

- [x] **M19.3** -- Sweep for the same shape elsewhere. The M18.1 audit asked what a returned value *permits*,
  not how long its borrow *lasts*; those are different questions and only the first was asked. Re-check every
  borrow-returning entry in [BORROW-SURFACE.txt](BORROW-SURFACE.txt) against the second, and record the
  distinction in [DESIGN-INSTRUCTIONS.md](DESIGN-INSTRUCTIONS.md) so the recurring question covers both.
  **Swept all seven entries; `get` was the only one.** `get_mut` never had it (`&mut self` already conflicts
  with the shared borrow `read_registered` needs -- confirmed by compiling the analogous sequence and getting
  `E0502`). `RingScope::batch` and `EventDelivery::scope` both borrow exclusively and confine what they hand
  out. `RingContract::violations` holds no kernel resource and every `observe_*` takes `&mut self`.
  `IoRingError::name` returns `&'static str` from a literal.
  [DESIGN-INSTRUCTIONS.md](DESIGN-INSTRUCTIONS.md) now poses **both** questions, with a mechanical form for
  the second -- take the borrow, then try to call everything that could start work against the same object --
  and D-45 is added to its table of shipped defects of this shape.
  **Swept the count restatements too:** that file said "three defects" in four places and is now four, which
  is the restatement drift the repository's own conventions warn about.

## M20 -- Repairs from the 2026-08-30 NUMA-sharding measurement

Queued from
[DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md](../../design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md),
which measured a shipping ARM laptop and found the L3 heuristic's justification does not hold there. These
are documentation and policy repairs only; **no defect was found in `ring_copy`** -- `Policy::select`
already degrades to a whole-machine domain and reports it, which an initial reading of the session got
wrong and the code corrected.

The design questions the session opened are deliberately **not** queued here. It is still open, and its
conclusions belong to it until it converges.

- [ ] **M20.1** -- Correct the L3 heuristic's justification in
  [DESIGN-NOTES.md](DESIGN-NOTES.md). It currently says the last-level-cache domain "is meaningful on Intel
  and ARM too, where the NUMA node often is not." **Measured counter-example:** a Snapdragon X2 Elite
  (X2E80100, Qualcomm Oryon; 12 cores, no SMT) reports **zero** L3 cache domains -- `L3CacheSize = 0` from
  WMI, and `GetLogicalProcessorInformationEx` yields L1 and L2 only, with L2 forming two domains of six
  processors that agree with the two `Module` domains. The claim that L3 is meaningful on ARM is false on a
  shipping part. Keep the finding that L3 beats the NUMA node; restate the rule as **the outermost cache
  level that actually partitions the machine**, and say what happens when no such level is reported. Sweep
  every restatement of the L3 rule per the repository's blast-radius convention, including the README and
  `ring_copy`'s `policy.rs` doc comments, not only the one sentence quoted above.

- [ ] **M20.2** -- Record the measurement itself as a decision in
  [DESIGN-NOTES.md](DESIGN-NOTES.md), so the next reader inherits the datapoint rather than re-measuring:
  an ARM Windows laptop with no L3 at all, and zero `Win32_NumaNode` instances, is the *common* consumer
  shape now rather than an exotic one. This is the ARM sibling of the existing zero-NUMA-node VM
  observation and belongs beside it.

- [ ] **M20.3** -- Make `ring_copy`'s degraded-fallback path observable in a test. The whole-machine
  fallback in `Policy::select` is the branch every zero-relation machine takes, and this session was the
  first time anyone confirmed it runs. Assert both halves on a synthetic topology: that a policy whose
  relation is absent returns one whole-machine domain with `degraded = true`, and that a policy whose
  relation is present is **not** flagged degraded -- the second half matters because a test of the first
  alone would pass against a function that always degrades.

## M6+ -- Model B: explicit-thread delivery and affinity

Parked, not pending. Deferred by the engineer's explicit direction during the 2026-08-22 design session,
with the plan scoped now so the shape is not lost. This is **not** a fallback for a missing capability
(D-3) -- it is the high-performance architecture, and M4's thread-pool path is the convenient one.

- [ ] **M6+.1** -- `DeliveryMode::{ThreadpoolWait, PinnedThread}` as an explicit consumer choice, never an
  automatic degradation.

- [ ] **M6+.2** -- Resolve the contention between a thread parked in `SubmitIoRing(ring, n, INFINITE, ..)`
  and callers wanting to build SQEs. This is the hard part and the reason this is its own milestone: it
  directly contradicts M3.1's `&mut`-enforced serialization, and needs either a submit-ownership handoff
  or an internal lock. Neither is obviously right.

- [ ] **M6+.3** -- Shutdown: waking a thread parked on `INFINITE`. `IORING_OP_NOP` is supported and is the
  wake mechanism.

- [ ] **M6+.4** -- Affinity: binding a ring's thread with `SetThreadGroupAffinity`, and documenting the
  execution-domain pattern (one pinned thread, its ring, its node-local registered pool, its shard).

- [ ] **M6+.5** -- A test seam forcing the pinned-thread path even where the completion event is available,
  so it stays testable on every machine rather than only on hardware that lacks the feature.

- [ ] **M6+.6** -- Decide `IoBuf`: extract to a shared crate, re-export from
  `windows-overlapped-io-sys`, or leave duplicated (D-1). The merge-or-delete decision that duplicate-then-decide
  defers to the point where the new path is proven -- which is here, not earlier.
