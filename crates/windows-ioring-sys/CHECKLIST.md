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

M19 is archived [here](COMPLETED-CHECKLIST.md#m19).

**`M20` through `M23` are pending; `M6+` is parked rather than pending** -- see the `M{n}+` convention: it
is gated work with no current obligation, not an unfinished milestone.

## M20 -- Repairs from the 2026-08-30 NUMA-sharding measurement

Queued from
[DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md](../../design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md),
which measured a shipping ARM laptop and found the L3 heuristic's justification does not hold there. These
were queued as documentation and policy repairs only, on the basis that **no defect was found in
`ring_copy`** -- `Policy::select` already degrades to a whole-machine domain and reports it, which an
initial reading of the session got wrong and the code corrected.

**Corrected 2026-09-19: that basis no longer holds, and it changes the order.** `SH-4.12` in
[CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md) later found two
defects in that same function: it selects on `DomainKind::Cache { level: 3, .. }` rather than asking
`outermost_partitioning_cache()` -- the one definition of which cache level partitions a machine, shipped
in `windows-topology-sys` 0.2.0 -- so it can produce **overlapping** ring domains where two cache kinds
report at level 3, and degrades silently on a host whose outermost partition sits at another level.
`M20.1` and `M20.3` both land on that function and that rule, so both are **coupled to `SH-4.12`** and
must follow it. `M20.2` and `M20.4` are done. `M20.6` is gated the other way, on `M22.1`. That leaves
`SH-4.12` as the only thing standing between M20 and completion.

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
  > **COUPLED TO `SH-4.12`** in [CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md)
  > -- do that item first. It rewrites `Policy::select` to ask `outermost_partitioning_cache()` and
  > **renames the policy**, since `byl3` is a user-facing CLI value that would no longer describe what it
  > does. This item's sweep reaches `policy.rs`'s doc comments, so running it first would make the doc
  > describe a rule the code below it does not implement -- the contradiction the blast-radius convention
  > exists to prevent.

- [x] **M20.2** -- Record the 2026-08-30 ARM measurement as a decision, beside the zero-NUMA-node
  observation it is the sibling of.
  -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#m202)

- [ ] **M20.3** -- Make `ring_copy`'s degraded-fallback path observable in a test. The whole-machine
  fallback in `Policy::select` is the branch every zero-relation machine takes, and this session was the
  first time anyone confirmed it runs. Assert both halves on a synthetic topology: that a policy whose
  relation is absent returns one whole-machine domain with `degraded = true`, and that a policy whose
  relation is present is **not** flagged degraded -- the second half matters because a test of the first
  alone would pass against a function that always degrades.
  > **COUPLED TO `SH-4.12`** in [CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md)
  > -- do that item first. It rewrites the selection arm this test would assert against, so writing the
  > test now pins behaviour that is about to change. The two halves this item asks for are the right
  > assertions either way; what changes is which rule "the policy's relation is present" names.

- [x] **M20.4** -- Correct "What is not reachable" in [DESIGN-NOTES.md](DESIGN-NOTES.md): the
  file-handle-to-storage-node mapping is reachable on mechanism, and the conclusion it supported now rests
  on volume granularity, absence, and spanned volumes instead.
  -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#m204)

- [x] **M20.5** -- Dissolved by [D-47](DESIGN-NOTES.md#d-47-detail) rather than decided: the
  `flush_barrier` assertion was measuring a claim the platform does not honour, so it was never a
  flaky test. -> [completed 2026-09-07](COMPLETED-CHECKLIST.md#m205)

- [ ] **M20.6** -- Re-evaluate `CommitStrategy::AlternatingRings` and the epoch-log benchmark's conclusion
  against [D-47](DESIGN-NOTES.md#d-47-detail). The strategy comparison in
  [strategy.rs](examples/epoch_log/strategy.rs) was designed around D-24's claim that a covering flush holds
  back operations queued behind it: the harness deliberately keeps appending while a commit is outstanding so
  that the stall would be visible in the numbers. D-47 established there is no such stall, so **the rationale
  the benchmark rests on is withdrawn even though the measurements themselves stand**. Two things to settle,
  and they are independent: whether alternating rings still earns its cost now that its stated benefit
  (keeping appends off a stalled ring) does not exist -- the remaining benefit is that epoch *N+1*'s appends
  are provably outside epoch *N*, which is a correctness property rather than a throughput one -- and whether
  the published numbers should be re-read, re-run, or annotated. **Not a documentation-only fix:** if the
  answer is that the strategy no longer earns its place, that is an API change to a published example.
  The corrected prose in [strategy.rs](examples/epoch_log/strategy.rs) and
  [DESIGN-NOTES.md](DESIGN-NOTES.md) both point here.
  **Addendum from the 2026-09-19 review** (`S-2` in
  [DESIGN-SESSION-2026-09-19-epoch-log-review.md](design-sessions/DESIGN-SESSION-2026-09-19-epoch-log-review.md)):
  the remaining benefit is stronger than "a correctness property". [D-47](DESIGN-NOTES.md#d-47) withdrew the
  hold-back half but kept the other -- the barrier still reaches *every* operation outstanding on the ring --
  so alternating rings bounds what a commit's barrier can be dragged into: on a shared ring commit latency is
  unbounded in unrelated traffic, and on alternating rings it is bounded by the epoch. That is a throughput
  argument after all, sited differently, and it is measurable with the harness that already exists. Unmeasured
  as of that session. Settle this **after** `M22.1`, whose per-record submit is a shared term in the numbers
  being re-read.
  *(Numbered M20.6 rather than M20.5 because M20.5 was in flight on a separate branch when this was
  written. That branch was closed unmerged; M20.5 arrives here instead, dissolved -- see above.)*


## M21 -- Epoch-log review: correctness repairs

Queued from
[DESIGN-SESSION-2026-09-19-epoch-log-review.md](design-sessions/DESIGN-SESSION-2026-09-19-epoch-log-review.md)
(findings `C-1` through `C-5`). Independent of each other; listed in ascending cost. Nothing in this
milestone was observed failing at the sample's current constants -- these are a withdrawn justification,
two hang shapes, a mis-keyed trigger, and a specification gap. (`M21.3` predicted that its trigger was
merely unreachable *today*; measuring it while implementing showed it is unreachable at any constants, so
what it corrected was the coupling rather than a latent bug. The archived entry has the numbers.)

- [x] **M21.1** -- Correct the last site that still asserts [D-24](DESIGN-NOTES.md#d-24)'s withdrawn
  half: the epoch-order assertion in the epoch-log committer, whose justification cited the hold-back
  claim [D-47](DESIGN-NOTES.md#d-47) removed.
  -> [completed 2026-09-20](COMPLETED-CHECKLIST.md#m211)

- [x] **M21.2** -- Publish a bounded pop and the wait it is generic over, then remove the two unbounded
  spins. `IoRing::pop_within` / `pop_within_with`, over a `CompletionWait` the caller supplies, because
  [D-21](DESIGN-NOTES.md#d-21) means the crate cannot choose the wait for them.
  -> [completed 2026-09-21](COMPLETED-CHECKLIST.md#m212)

- [x] **M21.3** -- Key the epoch commit off a completed append rather than off the counter, so the
  trigger cannot fire on a pass that appended nothing. The predicted latent bug turned out to be
  unreachable at any constants -- measured, not re-reasoned -- so this is a coupling change rather than
  a fix.
  -> [completed 2026-09-21](COMPLETED-CHECKLIST.md#m213)

- [x] **M21.4** -- State what a *failed* commit does to `durable_through`, and bind it with tests in both
  directions. Required making the sample a test target at all (`test = true`), and gating the
  failure-path tests on `fault-injection`, since a healthy flush cannot be made to fail.
  -> [completed 2026-09-21](COMPLETED-CHECKLIST.md#m214)

- [x] **M21.5** -- Give the harness's wait loops a bound, and collapse the hand-written waits onto the
  bounded pop. The item named two loops; a census found four, plus two flaky single-`try_pop` sites.
  -> [completed 2026-09-21](COMPLETED-CHECKLIST.md#m215)

- [x] **M21.6** -- Fix the four defects an independent review of the `M21.2` surface found: the timeout
  mapping, its victim in `run_down`, the `INFINITE` collision, and the test hole that hid all of them.
  -> [completed 2026-09-21](COMPLETED-CHECKLIST.md#m216)

## M21+ -- Queued by the 2026-09-21 API review

Queued from the review of the `M21.2` surface, recorded in
[DESIGN-SESSION-2026-09-21-m21-remediation-findings.md](design-sessions/DESIGN-SESSION-2026-09-21-m21-remediation-findings.md).
Its other four findings were fixed in `M21.6`.

- [x] **M21+.1** -- Teach [check-borrow-surface.ps1](../../tools/check-borrow-surface.ps1) the two shapes
  it was blind to: methods of a `pub trait`, and borrows in parameter position. Four entries appeared, one
  of them predating the widening; the probes also found a latent bug in the checker itself.
  -> [completed 2026-09-21](COMPLETED-CHECKLIST.md#m21plus1)

## M22 -- Epoch-log review: submission and arena

Queued from the same session (findings `E-1` through `E-3`). `M22.1` is sequenced first because `M20.6`
re-reads numbers that its change moves.

- [ ] **M22.1** -- Batch an epoch's appends into one submission, in both append paths (`E-1`).
  [`Appender::append`](examples/epoch_log/append.rs) and [`Lane::append`](examples/epoch_log/strategy.rs)
  each construct a `Batch`, push one write, and submit -- so the sample that exists to teach `Batch` never
  amortises a submission, which is what `Batch` is for. Two consequences, and the second is why this leads
  the milestone: the sample teaches the wrong shape, and the fixed per-record submission cost is a shared
  term in all three strategies of the M14.3 comparison whose headline result is that they are
  indistinguishable. Whether batching moves that spread is **unmeasured**; measure it, and record the figures
  in a committed capture the prose links to rather than pasted into two documents.

- [ ] **M22.2** -- Collapse the two free-slot implementations to one (`E-2`).
  [`Appender::free_slot`](examples/epoch_log/append.rs) scans the arena calling `outstanding()` per slot
  while `Lane` keeps a `Vec<u32>` free list. Both are correct and the cost difference is nil at eight slots;
  the duplication is the defect, because the two can drift. One definition, both callers bind to it.

- [ ] **M22.3** -- Give the registered arena a stated placement, or state why it has none (`E-3`).
  [append.rs](examples/epoch_log/append.rs) allocates it as `vec![0_u8; SLOT_LEN]` -- heap, no alignment, no
  node -- while [lib.rs](src/lib.rs) tells every consumer that buffer placement "is very likely the
  highest-leverage locality decision available" and names `VirtualAllocExNuma`, and
  [ring_copy/buffer.rs](examples/ring_copy/buffer.rs) already implements exactly that. Either adopt that
  allocator here or write down why a durability sample deliberately makes no locality decision. What is not
  acceptable is the current silence, which reads as an oversight and contradicts the crate's own front page.


## M22+ -- Queued by what the M21 work left behind

- [ ] **M22+.1** -- Make [bounded_pop.rs](tests/bounded_pop.rs) independent of how fast a device is.
  The tests need an operation still pending when a short bound expires, and they get it from a 128 MiB
  unbuffered, overlapped read -- which is a *margin*, not a guarantee, because `NO_BUFFERING` bypasses the
  system cache but not the drive's. One unidentified failure was observed and is recorded in
  [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md); it did not reproduce in 19 runs.
  The robust shape is an operation that **cannot** complete rather than one that is merely slow: a read on
  an overlapped named pipe nobody writes to, released by writing a byte when the test is done with it.
  Two unknowns to settle first, both cheap to answer and neither safe to assume: whether `IoRing` accepts a
  pipe handle for `read_raw` at all, and what adding `Win32_System_Pipes` to the dev-dependency feature set
  costs. If pipes do not work, the fallback is to keep the read but establish the pending state by bounded
  retry rather than by assertion, so a single fast run cannot fail the suite.
  Doing this also lets the fixture shrink from 128 MiB per test, which is the larger part of what this file
  costs to run.

## M23 -- The ring as a durability domain, and storage affinity

Queued from the same session (findings `S-1` and `S-3`). `S-2` is an addendum to `M20.6` rather than an item
here. `M23.2` builds on the mechanism correction `M20.4` carried, which has landed.

- [ ] **M23.1** -- Say in [contract.rs](examples/epoch_log/contract.rs) that the ring is part of the
  durability unit (`S-1`). [D-47](DESIGN-NOTES.md#d-47) withdrew the hold-back half of
  [D-24](DESIGN-NOTES.md#d-24) and kept the other: the barrier still reaches *every* operation outstanding on
  the ring, not only the current submission batch. So a commit's latency is a function of whatever else
  shares the ring, and "one ring per log" is a **precondition** of this sample's durability contract rather
  than a convenience of how it happens to be written. `contract.rs` is where this sample states its
  preconditions, and was deliberately written before the code; it does not currently say this.

- [ ] **M23.2** -- Record a decision on how a consumer anticipates storage affinity, given that the node
  question is unanswerable and the device question is not (`S-3`). Two mechanisms, both leaving policy with
  the consumer per [D-8](DESIGN-NOTES.md#d-8): (a) let a consumer **declare** a domain's storage node and
  have the arena allocate there with `VirtualAllocExNuma`, turning an undiscoverable fact into a stated
  input that [file-handle-numa-spike.rs](design-sessions/spikes/file-handle-numa-spike.rs) can fill in
  automatically if hardware ever answers; and (b) shard by **backing device** rather than by node, using
  `IOCTL_STORAGE_GET_DEVICE_NUMBER` and `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` -- both already named in that
  spike, both reachable today on ordinary hardware. The second is the substantive one: a device cache flush
  is per-device, so two logs on one device contend at every commit and a ring spanning two devices takes the
  slower device's flush on every covering flush, which means the reachable question is also the one that
  governs the cost this sample is built around. **Unmeasured** -- it follows from the flush's recorded scope
  plus D-47's surviving half, and the instruments to settle it exist. Decide what this crate offers, what it
  refuses, and what it measures first.




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
