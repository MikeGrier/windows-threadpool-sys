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
  > **`M22.1` has landed and its measurement is in.** The confound is **not supported**: removing
  > the per-record submission cost left the cross-strategy spread inside a single strategy's own
  > run-to-run range, so the "indistinguishable" conclusion survives on the grounds it already had.
  > Twenty runs, ten each side, in
  > [measurements/2026-09-22-append-batching/](measurements/2026-09-22-append-batching/). What
  > remains for this item is the part no number speaks to: whether alternating rings earns its cost
  > on **correctness and blast-radius** grounds, per `S-2` above.
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

**`M24.1` gates everything after it.** `M24.2` and `M24.3` are safe under any outcome and could be
taken first if the evaluation is deferred; `M24.4` exists only if `M24.1` says it may.

- [ ] **M24.1** -- Settle whether a fake whose assertions are **shared** with the kernel escapes the
  objection in [Two techniques deliberately rejected](DESIGN-NOTES.md#two-techniques-deliberately-rejected).
  That rejection refuses a mock because it "would have manufactured evidence" a kernel-behaviour bug
  was absent, and this pass added three fresh confirmations of it -- `ERROR_TIMEOUT` on an expired
  wait, `E_INVALIDARG` on a wait with nothing pending, and inline completion on a synchronous handle,
  each of which a hand-written fake would have got wrong.
  **Settle it by demonstration, not by argument**, because the argument is exactly what is in doubt.
  Build a throwaway fake with a *deliberately wrong* accounting model (decrement `outstanding` in the
  wrong place) and confirm the shared suite turns red on the fake side while the kernel side stays
  green. Then do the converse: give the fake a wrong *Windows* belief and confirm the shared suite
  does **not** catch it -- which is the expected result, and is why the bright line in
  [D-49](DESIGN-NOTES.md#d-49) exists rather than being a hedge. A co-tested peer is only defensible
  if both halves behave as predicted.
  Conclude by amending that decision or recording that it stands, and by checking `M24.4` off as
  withdrawn if it stands.

- [ ] **M24.2** -- Extract the handle-free accounting into its own type, composed by `IoRing`.
  Measured as separable: `RingId::next()` is a process-global `AtomicU64` that never touches a
  handle, and `IoRing`'s ten fields split evenly -- `ring_id`, `next_user_data`, `outstanding`,
  `registered_files` and `registered_buffers` carry no kernel state, against `handle`,
  `completion_event`, `registered_buffer_infos`, `version` and `supported_ops` which do.
  This is the remedy that needs **no fake, no feature gate and no widened visibility**: most of the
  38 lib tests that currently reach crate-private items become hermetic *in place*, because what
  they were always testing is bookkeeping rather than the kernel. Safe under any outcome of `M24.1`.
  Pure refactor of internals; the public surface does not move.

- [ ] **M24.3** -- Relocate the 25 lib tests that open a ring but use **only public API** into
  `tests/`. A pure relocation, and it carries the split provenance trail the repository requires of
  any move: `Split-Source` / `Split-Into` trailers, and a `git blame -w -C1 -C1` check that the moved
  lines still trace to their original commits rather than to the move.

- [ ] **M24.4** -- **Conditional on `M24.1`.** Build the shared conformance suite: bookkeeping tests
  written once as generic functions, run against a hermetic fake from `src/` and against the real
  ring from `tests/`. The suite must be reachable from both, so it is a `pub` module behind a
  non-default `test-util` feature -- the pattern `windows-file-watcher` already ships.
  **Accept the consequence explicitly rather than discovering it:** feature-gated code is invisible
  to a default `cargo test` and to `cargo mutants` without `--all-features`, and this repository has
  measured what that does -- a `windows-file-watcher` sweep reported 247 survivors of which 147 were
  in gated modules. CI's `--all-features` job must cover the suite, and any mutation run must pass
  the flag.

- [ ] **M24.5** -- Put the rule on a rung, so it cannot regress. After `M24.2` and `M24.3` the lib
  tests should construct no ring at all; assert that mechanically rather than by review -- a check
  that no `src/**/tests.rs` constructs an `IoRing`, wired into CI beside
  [check-borrow-surface.ps1](../../tools/check-borrow-surface.ps1).
  **Verify it in both directions**, per the bidirectional-guard rule: it must fire on a lib test that
  opens a ring, and stay silent on one that does not. A check that cannot fire is decoration, which
  `M21+.1` established this repository can ship without noticing.

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
