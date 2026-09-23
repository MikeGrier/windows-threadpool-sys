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

- [x] **M20.1** -- Restate the cache heuristic as "the outermost cache level that actually partitions
  the machine", sweep every restatement, and replace the consumer that bound to the level number.
  Done together with `SH-4.12`, which is the code half of the same change.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m201)

- [x] **M20.2** -- Record the 2026-08-30 ARM measurement as a decision, beside the zero-NUMA-node
  observation it is the sibling of.
  -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#m202)

- [x] **M20.3** -- Make `ring_copy`'s degraded-fallback path observable in a test, asserting both
  that an absent relation degrades and that a present one does not. Done without waiting on
  `SH-4.12`: the fallback tail is shared by every policy, so exercising it through `ByNode` and
  `ByPackage` pins nothing that item rewrites.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m203)

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
  > **`M22.1` has landed and its measurement is in.** The confound is **not supported**: removing
  > the per-record submission cost left the cross-strategy spread inside a single strategy's own
  > run-to-run range, so the "indistinguishable" conclusion survives on the grounds it already had.
  > Twenty runs, ten each side, in
  > [measurements/2026-09-22-append-batching/](measurements/2026-09-22-append-batching/).

  **Investigated 2026-09-22. Half of this item is answered; the other half needs `M25` first.**

  **Answered, structurally, and it needs no measurement.** Alternating rings cannot reduce the
  per-ring blast radius: `RegisteredBuffers::get_mut` refuses a slot with an operation outstanding
  and there are `SLOTS` slots, so at most `SLOTS` appends are outstanding on a ring **by
  construction** -- and each alternating lane registers its own arena of the same size. The bound is
  identical either way. Probing it agreed (8 and 8), but the argument does not rest on that, and it
  holds whatever the platform does about pending. So `S-2`'s "on a shared ring commit latency is
  unbounded in unrelated traffic" does not apply to this sample: the arena bounds it, not the ring
  topology. `S-2` would still apply against genuinely unrelated traffic from another component with
  its own buffers, of which this sample has none.

  **Blocked on `M25` for the rest**, because the numbers this item was to re-read do not measure what
  they are labelled: blocking p50 **and p99 are 0 us** for all three strategies, so the published
  commit-latency column is entirely deferral, and `AlternatingRings`' apparently-worse latency is an
  artifact of it settling on a two-epoch rotation against everyone else's one. Underneath that, the
  commit's `SubmitIoRing` took 289-555 us and returned with every completion already queued, so no
  overlap exists to differentiate the strategies at all. Re-reading, re-running or annotating these
  numbers cannot help; the harness has to change first, which is `M25`.
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

- [x] **M22.1** -- Batch an epoch's appends into one submission in both append paths, and measure
  whether the per-record submission cost was flattening the strategy comparison. It was not:
  throughput did not move out of the noise, though commit p50 did.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m221)

- [x] **M22.2** -- Collapse the two free-slot implementations to one, derived from the arena's own
  outstanding counts rather than tracked beside them. The item called both correct; one was not --
  the tracked free list leaked a slot on every refused append.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m222)

- [x] **M22.3** -- Give the registered arena a stated placement: the epoch-log arena is placed on the
  NUMA node its own log file's volume reports, and the allocator moved into the library as
  `NumaBuffer` rather than being copied a second time. The sample says plainly that the placement
  cannot pay at this workload.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m223)


## M22+ -- Queued by what the M21 work left behind

- [x] **M22+.1** -- Make [bounded_pop.rs](tests/bounded_pop.rs) independent of how fast a device is, by
  reading from an overlapped pipe nobody has written to. Filed and completed the same hour; the deferral
  was a scheduling preference rather than a blocker.
  -> [completed 2026-09-21](COMPLETED-CHECKLIST.md#m22plus1)



## M24 -- Make the unit suite hermetic

The defect and its classification are [D-49](DESIGN-NOTES.md#d-49); the remedies and their costs are
[DESIGN-SESSION-2026-09-21-hermetic-unit-tests.md](design-sessions/DESIGN-SESSION-2026-09-21-hermetic-unit-tests.md).
**63 of 131 lib tests open a real kernel ring**, so `cargo test --lib` does not mean what its name
implies, and the repository's own Quality rule already classifies an operating-system API as an
external boundary.

**Sequencing is the open question, not whether. Corrected 2026-09-22: the cost of waiting is close
to zero, which is the opposite of what this paragraph first said.** It claimed that waiting
compounds, "because every milestone that adds tests adds to the pile to be migrated, and `M22` is a
testing-heavy milestone". The mechanism is real but the instance was not checked, and it is false:
**all three `M22` items touch only `examples/epoch_log/`**, and none adds a lib test.

**Unconditional as of 2026-09-22.** `M24.1` concluded and `M24.4` is withdrawn, so nothing in this
milestone waits on an evaluation any more. The hermetic goal is reached by relocation and by the
accounting extraction alone; the technique `M24.1` went looking for turned out to be a different
and larger thing, and is `M26`.

**Recount the population before starting.** [D-49](DESIGN-NOTES.md#d-49) records 63 of 131 lib tests
opening a ring, and the suite has since grown -- `M22.3` added 14 `NumaBuffer` tests, which open
none. The 63 is the number to act on, but the denominator in any prose written during this milestone
must come from a command rather than from that decision.

Checked across the whole queue rather than for `M22` alone, since the first claim was wrong for
want of exactly that: **no pending item outside this milestone modifies `src/**/tests.rs`.** `M22`
is example-only; `M23.1` is the *sample's* `contract.rs`, not the crate's; `M20.1` and `M20.6` are
documentation and the `ring_copy` sample; `M23.2` is a decision that may imply API later. The 63
therefore do not grow while this waits.

So sequencing turns on other things, and they point the other way:

- **`M24.2` is an internals refactor of a published crate**, and the branch carrying this work is
  already 19 commits with one `feat` and three `fix` commits on it. Stacking a field-layout change
  on top makes one review cover both a new public API and that refactor.
- **`M24.1` is an evaluation whose answer could invalidate `M24.4`**, so beginning the build before
  it concludes risks building something the evaluation rejects.
- **`M22.1` unblocks `M20.6`**, an open question since 2026-09-07 about whether a strategy still
  earns its place in a published sample -- which is a decision waiting on a measurement `M22.1`
  produces.

**`M24.1` concluded (2026-09-22) and nothing here waits on it.** `M24.4` is withdrawn; `M24.2`,
`M24.3` and `M24.7` are the path to a hermetic suite and are independent of each other.

- [x] **M24.1** -- Settle whether a co-tested fake escapes the mock objection. **Answered: the fake
  was the wrong instrument.** A shared suite is strong over what we specify and blind to the
  platform's incidental behaviour, and an assertion about the latter is a frozen observation rather
  than a contract. Superseded by the resolver in `M26`.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m241)

- [x] **M24.2** -- Extract the handle-free accounting into its own type, composed by `IoRing`. The
  item's field split was verified exactly: five fields carry no kernel state, five do.
  `Accounting` now owns them with 19 hermetic tests, and `IoRing` delegates nine methods.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m242)

- [x] **M24.7** -- Convert the lib tests that construct a ring only to exercise bookkeeping.
  **61 -> 52**, by narrowing `Token::new` to take the ring's ledger rather than the ring. The
  remaining 52 are not convertible and the reason is structural, not effort -- see the archive.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m247)

- [x] **M24.4** -- **Withdrawn by `M24.1` (2026-09-22).** A shared conformance suite over a
  hand-written fake is superseded by the response-space resolver in `M26`, which serves the same
  purpose without encoding a belief about the platform at all. Nothing is deferred by this: `M24`'s
  goal is a hermetic lib suite, and `M24.2` plus `M24.3` achieve that without it.

- [x] **M24.3** -- Relocate the lib tests that open a ring but use only public API into `tests/`.
  **52 -> 41.** Eleven moved; the "25" the item predicted was never achievable, and the reason is
  the same structural one `M24.7` found.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m243)

- [x] **M24.5** -- Put the rule on a rung. An **inventory** of which lib tests open a ring
  ([D-53](DESIGN-NOTES.md#d-53)), not the zero-check the item assumed -- that rule is false and
  could only be satisfied by deleting coverage. The guard's own bidirectional check found a defect
  in the guard.
  -> [completed 2026-09-22](COMPLETED-CHECKLIST.md#m245)

- [ ] **M24.6** -- Sweep what this milestone makes false. The testing-strategy section of
  [DESIGN-NOTES.md](DESIGN-NOTES.md#testing-strategy-m185) describes which population each technique
  reaches and was written when every lib test opened a ring; `M21.6`'s archive entry says the M21.2
  tests "drive the loop with a wait that never enters the kernel", which stops being the notable
  exception once the suite is hermetic; and the
  [2026-09-21 remediation ledger](design-sessions/DESIGN-SESSION-2026-09-21-m21-remediation-findings.md)
  carries `F-13`, whose "the crate's tests never exercise asynchronous completion" is a claim about
  the structure this milestone changes. Count the restatements with a command, not by eye.

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
  have the arena allocate there, turning an undiscoverable fact into a stated
  input that [file-handle-numa-spike.rs](design-sessions/spikes/file-handle-numa-spike.rs) can fill in
  automatically if hardware ever answers; and (b) shard by **backing device** rather than by node, using
  `IOCTL_STORAGE_GET_DEVICE_NUMBER` and `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` -- both already named in that
  spike, both reachable today on ordinary hardware. The second is the substantive one: a device cache flush
  is per-device, so two logs on one device contend at every commit and a ring spanning two devices takes the
  slower device's flush on every covering flush, which means the reachable question is also the one that
  governs the cost this sample is built around. **Unmeasured** -- it follows from the flush's recorded scope
  plus D-47's surviving half, and the instruments to settle it exist. Decide what this crate offers, what it
  refuses, and what it measures first.

  **Narrowed by `M22.3` (2026-09-22), which settled the sample-level half of (a).** The allocation now
  exists in the library as `NumaBuffer` ([D-51](DESIGN-NOTES.md#d-51)), and the epoch-log sample already
  asks the FSCTL and places on the answer ([D-50](DESIGN-NOTES.md#d-50)). So (a) is no longer "should a
  sample do this" but the narrower **library** question: does the crate offer a *declared* storage node as
  an input anywhere, or does it stay at "you allocate, you choose"? (b) is untouched and is still the
  substantive one.

- [ ] **M23.3** -- Decide whether the crate offers a **pending-operations map**, and separately whether it
  offers a **slot arena** on top of one. Record the decision either way; if it is "yes", the
  implementation is spawned as its own items.

  **Replaces `M22+.2`, which asked a smaller question and justified it with the wrong evidence.** That
  item proposed `RegisteredBuffers::quiet()` and cited `M22.2`'s slot leak as the reason. The leak was in
  the hand-maintained *tracking* -- `free.pop()` before composing, verified against `7c12708e` -- not in
  the free-slot query, so `quiet()` would not have prevented the bug that justified it. `outstanding()`
  already carries that affordance and says so in its own rustdoc; both consumers had it and hand-rolled a
  free list anyway. **Do not re-propose `quiet()` without new evidence.**

  **What the duplication actually is, from a census of the tree rather than recollection.** Nine sites
  keep a map from `UserData` to an unclaimed `Token`, and claim it when the matching completion is
  popped:
  [completion_event.rs](tests/completion_event.rs), [event_delivery.rs](tests/event_delivery.rs),
  [flush_barrier.rs](tests/flush_barrier.rs), [flush_barrier_stress.rs](tests/flush_barrier_stress.rs),
  [handover.rs](tests/handover.rs), [submission_lifecycle.rs](tests/submission_lifecycle.rs),
  [checkpoint.rs](examples/epoch_log/checkpoint.rs), [append.rs](examples/epoch_log/append.rs), and
  [strategy.rs](examples/epoch_log/strategy.rs). Two of them independently declare a `type Pending` alias
  carrying the **same doc comment**, which is as strong a signal as this tree offers that the construct
  wants to exist once. Two of the nine -- `append.rs` and `strategy.rs` -- add registered slots on top,
  and those two are the slot arena.

  **The questions, in order.** (1) Is the pending map a library type, a documented pattern, or neither?
  It is where the claim discipline lives, and dropping a token unclaimed is the failure `Token`
  deliberately treats as still-outstanding -- so an abstraction here is an abstraction over a safety
  rule, which argues for it and also raises the bar. (2) Does a slot arena follow, or is it sample
  policy like partitioning is under [D-8](DESIGN-NOTES.md#d-8)? (3) If either is offered, what does it
  refuse to decide -- batching, ordering, and which slot to pick are all caller questions.

  **Counter-argument to answer, not dodge:** six of the nine sites are tests, and test convenience is a
  weak reason to grow permanent public surface. A `test-util` module, or nothing at all, may be the right
  answer. The contrast to hold it against is `NumaBuffer` ([D-51](DESIGN-NOTES.md#d-51)): roughly ninety
  lines of unsafe FFI, RAII and trait impls a caller cannot obtain any other way, which is a different
  proposition from a `HashMap` a caller can write in three lines.




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


## M25 -- Make the epoch-log sample's I/O a shape where a commit is observable

Queued by the `M20.6` investigation, which found three things the item did not anticipate.

**The harness measures the wrong quantity.** Decomposing its commit latency into *deferral* (flush
pushed -> harness next looked) and *blocking* (time actually waiting) gave blocking p50 **and p99 of
0 us for all three strategies**. The published `commit p50/p99/max` column is entirely deferral: it
reports how long the next epoch's appends took, not anything about the commit.

**There is no pipeline to measure.** The commit's `SubmitIoRing` took 289-555 us and returned with
all 9 completions already queued. The handle has no `FILE_FLAG_OVERLAPPED`, so the batch ran inline,
and the comment in [strategy.rs](examples/epoch_log/strategy.rs) reading "a real log keeps appending
while a commit is outstanding" describes something that cannot happen there.

**`AlternatingRings`' blast-radius claim is answered structurally, and needs no run.**
`RegisteredBuffers::get_mut` refuses a slot with an operation outstanding and there are `SLOTS`
slots, so at most `SLOTS` appends are outstanding on a ring **by construction** -- and each
alternating lane registers its own arena of the same size. The per-ring bound is identical either
way. Measured at 8 and 8, but the argument does not rest on the measurement, and it holds whatever
the platform does about pending.

[write-pending-spike.rs](design-sessions/spikes/write-pending-spike.rs) then established which
configurations pend at all. `FILE_FLAG_OVERLAPPED` alone changed nothing (0/500). Only
`NO_BUFFERING` over a **pre-written extent** pended reliably, and its submit p50 fell from ~500 us to
116 us -- the flush's cost leaving the submit path is what makes a commit separately observable for
the first time.

**A standing constraint on every item below.** That 500/500 is an observation, not a contract:
Windows specifies nothing about when a ring operation completes relative to `SubmitIoRing`. So the
sample may *adopt* this shape -- it is what real write-ahead logs do, and it is the only shape where
the measurement means anything -- but **nothing here may depend on an operation pending.** Every item
must leave the log correct if the platform completes inline tomorrow.

- [ ] **M25.1** -- Give records a fixed sector stride, and zero the slot tail before writing.
  `NO_BUFFERING` requires sector-aligned offsets *and* lengths, and records are variable-length at
  packed offsets today. One record per `SLOT_LEN`-sized, sector-sized block is the simplest stride
  that fits the existing arena. This **reverses a deliberate decision** in
  [append.rs](examples/epoch_log/append.rs) -- "writing the slot's unused tail would put stale bytes
  in the log and cost real device bandwidth" -- so the tail must now be zeroed rather than left, and
  the write amplification (a ~50-byte record occupying 4096) is a real cost to state rather than hide.
  It is also what a real WAL pays for sector atomicity.

- [ ] **M25.2** -- Advance replay by the stride rather than by `total_len`, and tolerate the
  pre-allocated zero tail. [replay.rs](examples/epoch_log/replay.rs) walks `cursor += found.total_len`.
  A pre-allocated log also ends in zeros rather than at EOF, so a **clean** log will now stop with
  `tail_stopped: Some(..)` where it previously ran out of bytes. `is_clean()` only inspects violations
  so cleanliness is unaffected, but the sample's printed narrative says "stopped at" and must be
  re-read. Both replay paths -- the normal one and the torn-tail one -- and the negative control have
  to keep meaning what they claim.

- [ ] **M25.3** -- Pre-allocate the log and open it `NO_BUFFERING | OVERLAPPED`. Create and size the
  file with an ordinary handle, drop it, then open the ring's handle over the existing extent -- the
  spike's condition D, and the only one that pended. The arena needs no change: `NumaBuffer` is
  page-granular from `M22.3`, which is at least sector-granular. Do the same for the strategy
  harness's own files. **Verify by sabotage that the pre-allocation is load-bearing**, since an
  extending `NO_BUFFERING` write pends only ~1% of the time and would otherwise look like it works.

- [ ] **M25.4** -- Measure the commit, now that there is one to measure. Report the flush's own
  duration rather than the deferral window, and keep the deferral visible as its own number so the
  two cannot be confused again. Whatever is reported must still be meaningful if an operation
  completes inline, per the standing constraint above -- so the harness reports what it observed,
  never assumes an overlap it did not get.

- [ ] **M25.5** -- Re-run the three-way comparison and answer `M20.6` on the numbers it was always
  meant to rest on. Commit the capture under [measurements/](measurements/) and link rather than
  paste it. The structural answer on blast radius stands whatever this shows; what is open is whether
  `AlternatingRings` earns its permanent doubled registration on any other ground.

- [ ] **M25.6** -- Sweep what this milestone makes false. At least: the "keeps appending while a
  commit is outstanding" rationale in [strategy.rs](examples/epoch_log/strategy.rs), that module's
  "what the measurement found" section, the `M22.1` capture's commit-p50 claim in
  [measurements/2026-09-22-append-batching/](measurements/2026-09-22-append-batching/) -- which this
  investigation showed was the deferral window shrinking because appends got faster, i.e. the same
  fact as the throughput result reported as unmoved -- and any DESIGN-NOTES text describing the
  sample's I/O as buffered. Record the findings above as decisions in the same pass.


## M26 -- Test against the space of kernel responses, not one observation of it

Queued by
[DESIGN-SESSION-2026-09-22-kernel-response-space.md](design-sessions/DESIGN-SESSION-2026-09-22-kernel-response-space.md),
which set out to answer `M24.1` and found a different technique instead.

**The idea.** A fake that models *what Windows does* freezes one run's testimony. A **resolver**
models what Windows is *permitted* to do, and a seed picks one resolution out of that space: which
operations finish inside `SubmitIoRing` and which pend, in what order completions are posted, which
fail. The assertions are then about **us** -- does this crate behave correctly under that resolution
-- and never about the kernel. There is no belief to be wrong about, which is why this dissolves the
mock objection rather than working around it.

**Justified by what it catches, not by hermeticity.** `M24` reaches a hermetic lib suite without it,
so this milestone has to earn its place on the defect class it detects: code that is brittle to
platform variation *inside* the permitted space. Nothing in the current toolkit detects that --
[DESIGN-NOTES.md](DESIGN-NOTES.md#what-none-of-them-cover) records that all five existing techniques
check this crate against *its own stated contract*.

**The standing constraint, inherited from the session.** The permitted space must be **wider than
anything observed**, and must not be derived from observation -- deriving it from what we have seen
closes the trap again. It is a deliberate specification of what we will tolerate, and therefore a
reviewable artifact rather than a recording.

- [ ] **M26.1** -- Specify the permitted response space, and record it as a decision. What may a
  submitted batch do? At minimum: each operation may complete inside `SubmitIoRing` or pend;
  completion order is unconstrained; an operation may fail individually; a wait may expire; a wait
  may return with nothing poppable. **Say equally which constraints hold**, because a resolver free
  to violate everything makes us write code defending against impossible kernels --
  [D-23](DESIGN-NOTES.md#d-23)'s covering-flush guarantee held with zero failures in ~4,500 trials,
  and whether the resolver may break it is a decision, not a default. Cite the spike or decision
  behind every entry, and mark the ones that are deliberate over-provision rather than observation.

- [ ] **M26.2** -- Build the seam. The resolver sits under the `windows-sys` calls -- `SubmitIoRing`,
  `PopIoRingCompletion`, the `Build*` family -- so those become indirect. **This is the expensive
  item and the one that touches a published crate's internals**; it is substantially more than
  `M24.2`'s field split. Do `M24.2` first: it is smaller, independently useful, and will show how
  much of `IoRing` separates cleanly before this commits to a shape.

- [ ] **M26.3** -- Build the resolver over the space `M26.1` specifies, seeded the way
  [generated_sequences.rs](tests/generated_sequences.rs) already is ([D-41](DESIGN-NOTES.md#d-41)):
  one number replays a whole run, announced with the command to replay it, pinnable from the
  environment. Keep that file's two-seed discipline in mind -- this adds a third axis, and
  conflating them would produce a replay that reproduces some of a run and not the rest.

- [ ] **M26.4** -- Write the properties that must hold under **every** resolution: conservation (no
  lost, duplicated or unclaimed completion), no hang, `pop_within` honours its bound, `outstanding`
  is accurate, no use-after-free. [`RingContract`](src/contract.rs) already states most of this as
  an oracle over observed sequences and should be the definition rather than a second copy.

- [ ] **M26.5** -- **Calibrate it, or it is not evidence.** Re-inject the two historical defects and
  confirm the resolver turns red: [D-47](DESIGN-NOTES.md#d-47)'s assumption that a covering flush
  holds back subsequent operations, and `M21.6`'s treatment of an expired wait as a failure. The
  session argued both by analogy from a demonstration and **deliberately did not claim them as
  measured**. `D-41`'s corollary is the rule: a green result from an instrument nobody has shown
  can go red is not evidence. This session produced two apparatus failures of exactly that kind.

- [ ] **M26.6** -- Point the kernel tests at their new job: confirming that reality stays **inside**
  the declared space, rather than re-checking behaviour the resolver already sweeps. A real kernel
  observed outside the space is a genuine finding and should fail loudly; a kernel that moves
  *within* it should change nothing. Sweep what this makes false, including the testing-strategy
  section's "five techniques" framing, which becomes six.

- [ ] **M26.7** -- Audit the existing suite for assertions that are **frozen observations rather
  than contracts** -- the failure case 4 of the session demonstrated, where one assertion gave
  opposite answers on two handles of the same API. [flush_barrier.rs](tests/flush_barrier.rs) is the
  obvious first candidate, being the direct descendant of `D-47`, but the audit is the point and not
  that file. For each, decide: restate as this crate's own contract, move to a spike with a rate and
  a date, or delete. **Not yet started, and not yet even sampled** -- the candidate above is a guess,
  and the census must come from a command.
