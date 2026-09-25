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

- [x] **M20.6** -- Re-evaluate `CommitStrategy::AlternatingRings` and the benchmark's conclusion
  against [D-47](DESIGN-NOTES.md#d-47-detail). **The harness cannot exhibit a blast-radius
  difference, which is a fact about the harness and not a finding against the strategy** -- each
  lane's own arena is the limiter there. The strategy stays, with the conditions under which it
  would pay written down; `M25.5` re-runs the comparison where operations genuinely pend. The
  sample's output and prose are corrected so they stop claiming to measure a commit.
  -> [completed 2026-09-23](COMPLETED-CHECKLIST.md#m206)


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
It began at **63 of 131 lib tests opening a real kernel ring**, so `cargo test --lib` did not mean
what its name implies, and the repository's own Quality rule already classifies an operating-system
API as an external boundary.

**Where it ended: 41 of 151 open a ring, 110 do not.** The remainder is not movable without
`M26.2`'s FFI seam, and [D-53](DESIGN-NOTES.md#d-53) records the rung that keeps it from climbing
back -- an inventory of *which* tests open a ring, since a zero-check would fail on day one and
could only be satisfied by deleting coverage.

**Sequencing is the open question, not whether. Corrected 2026-09-22: the cost of waiting is close
to zero, which is the opposite of what this paragraph first said.** It claimed that waiting
compounds, "because every milestone that adds tests adds to the pile to be migrated, and `M22` is a
testing-heavy milestone". The mechanism is real but the instance was not checked, and it is false:
**all three `M22` items touch only `examples/epoch_log/`**, and none adds a lib test.

**Unconditional as of 2026-09-22.** `M24.1` concluded and `M24.4` is withdrawn, so nothing in this
milestone waits on an evaluation any more. The hermetic goal is reached by relocation and by the
accounting extraction alone; the technique `M24.1` went looking for turned out to be a different
and larger thing, and is `M26`.

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

- [x] **M24.6** -- Sweep what this milestone makes false. Two of the three sites the item named
  were false alarms; the third was false for a different and larger reason than the item gave, and
  the sweep found two more it did not name.
  -> [completed 2026-09-23](COMPLETED-CHECKLIST.md#m246)

## M23 -- The ring as a durability domain, and storage affinity

Queued from the 2026-09-19 epoch-log review (findings `S-1` and `S-3`). `S-2` is an addendum to `M20.6`
rather than an item here. `M23.1` and `M23.2` are done; `M23.3` is the remaining question, and it is
about this crate's own surface rather than about storage at all.

- [x] **M23.1** -- State in the epoch-log contract that the barrier is ring-wide while the flush names a file, so one ring per log is a precondition of the cost model. -> [completed 2026-09-23](COMPLETED-CHECKLIST.md#m231)

- [x] **M23.2** -- Decide how a caller arrives at a NUMA node: `win-numa-sys` offers declaring and discovering, and refuses the shortcut that does both at once. -> [completed 2026-09-23](COMPLETED-CHECKLIST.md#m232)

- [x] **M23.3** -- Decide what this crate offers for holding a token between push and completion: the ring owns the inventory, `IoRing` becomes generic, and the break is accepted. -> [completed 2026-09-23](COMPLETED-CHECKLIST.md#m233)

- [x] **M23.4** -- Drop guards that panicked during unwind aborted the process instead of reporting; they now stay silent while `std::thread::panicking()`. -> [completed 2026-09-23](COMPLETED-CHECKLIST.md#m234)

- [x] **M23.5** -- Both asserts in `IoRing::drop` are now reached by tests; the raw-HRESULT seam the item priced turned out not to be needed, because the kernel refuses a null ring handle cleanly. -> [completed 2026-09-23](COMPLETED-CHECKLIST.md#m235)


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

- [x] **M26.1** -- The permitted space is specified in [RESPONSE-SPACE.md](RESPONSE-SPACE.md) as eleven cited clauses, and recorded as [D-59](DESIGN-NOTES.md#d-59). -> [completed 2026-09-24](COMPLETED-CHECKLIST.md#m261)

- [x] **M26.2** -- The eight submission-path `windows-sys` calls are indirect through [sys.rs](src/sys.rs), answerable by a thread-local responder behind the `kernel-seam` feature; the shape is recorded as [D-60](DESIGN-NOTES.md#d-60). -> [completed 2026-09-24](COMPLETED-CHECKLIST.md#m262)

- [x] **M26.3** -- The resolver is built over the space's clause IDs in [resolver.rs](src/sys/resolver.rs), seeded on its own axis, with permissions configurable and constraints not; recorded as [D-61](DESIGN-NOTES.md#d-61). -> [completed 2026-09-24](COMPLETED-CHECKLIST.md#m263)

- [x] **M26.4** -- The five properties are stated in [properties_under_every_resolution.rs](tests/properties_under_every_resolution.rs), with [`RingContract`](src/contract.rs) asked for the conservation verdict rather than copied; recorded as [D-62](DESIGN-NOTES.md#d-62). -> [completed 2026-09-24](COMPLETED-CHECKLIST.md#m264)

- [x] **M26.5** -- Both historical defects are re-injected and confirmed to turn the instrument red in [calibration.rs](tests/calibration.rs) and [sabotage.json](sabotage.json); recorded as [D-63](DESIGN-NOTES.md#d-63). -> [completed 2026-09-24](COMPLETED-CHECKLIST.md#m265)

- [ ] **M26.6** -- Point the kernel tests at their new job: confirming that reality stays **inside**
  the declared space, rather than re-checking behaviour the resolver already sweeps. A real kernel
  observed outside the space is a genuine finding and should fail loudly; a kernel that moves
  *within* it should change nothing. Sweep what this makes false, including the testing-strategy
  section's "five techniques" framing, which becomes six.

  **`RS-C-4` is the clause that makes this job real rather than nominal.** The resolver is forbidden
  to break the drain half of `DRAIN_PRECEDING_OPS`, so a Windows that broke it would be caught by
  nothing the resolver does -- these tests are where that would surface, and the division of labour
  is stated in [RESPONSE-SPACE.md](RESPONSE-SPACE.md) on the strength of it. Make sure at least one
  test actually exercises that clause against a real ring, or the constraint is untested in both
  halves at once.

- [ ] **M26.7** -- Audit the existing suite for assertions that are **frozen observations rather
  than contracts** -- the failure case 4 of the session demonstrated, where one assertion gave
  opposite answers on two handles of the same API. [flush_barrier.rs](tests/flush_barrier.rs) is the
  obvious first candidate, being the direct descendant of `D-47`, but the audit is the point and not
  that file. For each, decide: restate as this crate's own contract, move to a spike with a rate and
  a date, or delete. **Not yet started, and not yet even sampled** -- the candidate above is a guess,
  and the census must come from a command.

- [ ] **M26.8** -- Decide what [`IoRing::run_down`](src/ring.rs) should do when a submit it makes is
  **refused**. Found by `M26.3`'s resolver on its first contact with a real ring, and queued rather
  than fixed there because it is a decision and not a correction.

  **The observation.** Under `RS-P-7` a submit may be declined. `run_down` passes the result through
  `wait_outcome(hr)?`, which is `Ok` only for an expired wait, so a refusal returns `Err` with
  `outstanding() > 0` -- and `Drop` then asserts and calls `CloseIoRing` anyway. That is the same
  shape `M21.6` fixed for `ERROR_TIMEOUT`, reachable again through a different `HRESULT`. Measured:
  32 seeds pass with `may_fail_submits` off; seed `0x1A` fails at `0x80070008` with it on.
  [resolver_over_a_real_ring.rs](tests/resolver_over_a_real_ring.rs) pins the current behaviour in
  `a_declined_submit_reaches_run_down_as_an_error`, so whichever way this is settled, that test is
  what has to change.

  **Why it is a decision.** `run_down`'s own documentation argues that blocking is the safe failure
  mode and closing early is not, which says it should keep looping through a refusal. But a
  permanently failing submit then hangs, and "no hang" is one of the properties `M26.4` writes.
  The two pull opposite ways and neither is obviously right, so an engineer chooses: keep looping,
  loop a bounded number of times, or offer the caller a documented recovery route -- today there is
  none, since the operations stay outstanding and the ring cannot be safely dropped.

  **Note one way this was narrowed and should not stay narrow.** `M26.3`'s resolver declines a
  submit only when something is staged, which is *narrower* than the space requires -- nothing in
  [RESPONSE-SPACE.md](RESPONSE-SPACE.md) says a submit carrying no new work cannot fail. Widening it
  is part of answering this item, because a rundown submit with an empty staging area is exactly the
  case a real `ERROR_NOT_ENOUGH_MEMORY` would hit.

  **`M26.4` found the second face of this question, and it is about the document.**
  [RS-P-7](RESPONSE-SPACE.md) is written as a *consequence* clause -- "**if** `SubmitIoRing` fails,
  operations already built remain queued" -- and cites [D-5](DESIGN-NOTES.md#d-5), which establishes
  the no-rewind consequence and nothing about submits failing spontaneously. `M26.3`'s resolver read
  it as a *permission* to fail submits, and that reading is what produced this item. **The space
  never states that a submit may fail at all**, which is a gap rather than a decision: submits
  demonstrably can fail on a real system, so a space that omits it is narrower than reality in a
  place nobody chose. Answering this item therefore means deciding both halves together -- whether
  the space says a submit may fail, and what `run_down` does when one does -- because the second
  cannot be settled while the first is unstated.

  **What `M26.4` established in passing, so it need not be re-derived.**
  [`IoRing::pop_within`](src/ring.rs) surfaces the same refusal as an `Err`, and that one is *within
  its documented contract* ("returns any error from `SubmitIoRing`"), so a correct consumer retries
  -- [properties_under_every_resolution.rs](tests/properties_under_every_resolution.rs) does exactly
  that and recognises a refusal by asking the resolver rather than by matching a code.
  `run_down` is the one with no recovery route, which is what makes it the item.

- [ ] **M26.9** -- Find and fix the intermittent `Timeout` in
  [event_delivery.rs](tests/event_delivery.rs)'s two threadpool-delivery tests, recorded in
  [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md).

  **Why this is not merely a flaky test to re-run.** Measured at 1 failure in 80 runs of the
  compiled binary. The sabotage harness runs the whole suite once per case and the manifest holds
  41, so that rate gives roughly a **40% chance of a corrupted sweep** -- and the corruption falsely
  reports `caught`, which is a sabotage the suite did not catch being recorded as a clean bill of
  health. The harness is this repository's mechanism for keeping earlier guarantees checked; a 40%
  chance of a silent false pass undermines every conclusion drawn from it.

  **What has already been ruled out, so it is not re-tried:** ring-resource pressure (zero failures
  after roughly 18,000 ring create/close cycles), the widened seeded sweeps (zero after repeated
  property-suite and calibration runs), and CPU starvation (zero under a concurrent `cargo build`
  saturating the machine). The cause is genuinely unknown, which is why this is an investigation
  rather than a one-line timeout bump -- raising the five-second bound would hide it, and the
  question worth answering is whether a real consumer's completion can be delayed this way.

  **A cheaper interim mitigation exists and is a separate decision:** the harness could treat a
  failure in these two tests as *inconclusive* rather than as `caught`, which would stop the false
  clean bills without pretending the behaviour is understood.

## M27 -- What this crate owes the topology planner

**Re-planned 2026-09-23, the same day it was written.** M27 was originally "Adaptivity: the benefit
without the architectural commitment", and asked whether *this crate* should derive a partition for a
consumer who expresses no preference. That was the wrong owner, and the checklist rules require
saying so rather than quietly rewriting it. The adaptivity the
[adoption thesis](../../DESIGN-NOTES.md#the-adoption-thesis) asks for is delivered by
[topology-planner](../topology-planner/COMPONENT.md), which takes a dataflow description of the
application and returns one or more suggested realizations
([EP-D-6](../topology-planner/DESIGN-NOTES.md#ep-d-6)). Had the original M27.1 been answered here it
would have grown a second, weaker policy surface beside the one that component exists to provide --
the `outermost_partitioning_cache` defect again, where a policy answer lands in a crate whose job is
something else.

> **-> CROSS-COMPONENT PREREQUISITE:** `M27.1` and `M27.2` are gated on component
> `crates/topology-planner` -> `M1+` -> `EP-1+.5` and `EP-1+.6`, which decide the plan vocabulary
> this crate would be realized from. See [CHECKLIST.md](../topology-planner/CHECKLIST.md).

**What survives here is the realization end, not the policy end.** The planner emits a plan; the
outward adapter realizes it as buffers, rings and threads
([EP-D-5](../topology-planner/DESIGN-NOTES.md#ep-d-5)). That adapter is a separate crate, but it can
only build what this crate exposes, and nothing has ever checked that what it exposes is sufficient.
[D-8](DESIGN-NOTES.md#d-8) is untouched by all of this: policy stays out of this crate, and being
*constructible from* a policy decision made elsewhere is the opposite of taking one.

- [ ] **M27.1** -- **Census what a realizer would need from this crate, against the plan vocabulary,
  and name what is missing.** A plan states which processor a domain pins to, which memory node its
  pool allocates from, how many queues of which types, and where each channel's buffer lives. Walk
  each of those to the public API that would realize it and record the gaps. `NumaBuffer`
  ([D-51](DESIGN-NOTES.md#d-51)) is one half of the pool answer and arrived this month; the ring's
  own construction takes no placement input at all. **The output is a gap list, not an API** --
  proposing surface before the plan vocabulary is settled would be binding to a draft.

- [ ] **M27.2** -- **Gated on `M27.1` and on the planner's `EP-1+.6`.** Close the gaps the census
  names, as ordinary capability on this crate with no policy attached. Each gap is an input a caller
  supplies, never a choice this crate makes. Verify the way the thesis demands rather than the
  convenient way: construct from a plan built against a *synthetic* machine, since the planner is
  mockable by construction and this crate should be realizable without the hardware the plan
  describes.

- [ ] **M27.3** -- Give a consumer the means to answer placement questions on their own hardware.
  **Not gated on the planner** -- it is the client-side half of the thesis, and it is what lets a
  developer disagree with any plan they are handed. `cache_domains.rs` now prints every cache level
  beside the heuristic's pick; the equivalent for placement is a sample that reports what a chosen
  arrangement costs and what the alternatives would have cost, on the machine in hand.
  [ring_copy](examples/ring_copy) is the natural host, being already policy-selectable. **Do not ship
  a verdict** -- report the observation and let the consumer conclude, per OPTION INTEGRITY.

## M28 -- The ring owns the pending inventory

Queued by [D-55](DESIGN-NOTES.md#d-55), taken 2026-09-23 after the `M23.3` exploration. The break
is accepted deliberately: `IoRing` becomes generic so the inventory cannot drift from the ring,
because a consumer never holds a token to lose. The exploration and everything it falsified is in
[DESIGN-SESSION-2026-09-23-pending-inventory.md](design-sessions/DESIGN-SESSION-2026-09-23-pending-inventory.md);
`src/pending.rs` is the working spike and is the shape the internal map starts from.

**Sequenced so each step compiles.** The published crate is at 0.3.1, so this is a major bump and
every consumer names the type -- which means the migration order matters more than usual.

- [ ] **M28.1** -- **Decide what a caller receives, before writing any of it.** If the ring owns
  the token then `Batch::write` can no longer hand one back, and the shape of what replaces it is
  the whole design: an identity the caller matches later, or a claim that returns `(T, X)`
  directly from the ring. The second makes drift impossible and is the point of the break; the
  first is a smaller change that may not be worth breaking for. Settle it with the
  `Token::claim_if` safety argument in hand, since that is what currently makes a mismatched
  completion unclaimable.

- [ ] **M28.2** -- **Bound `RingContract` before anything depends on it more heavily.**
  `operations: HashMap<usize, State>` is never pruned -- `observe_claim` marks an entry
  `Completed` and keeps it -- so the oracle retains one entry per operation for the process's
  life. Undocumented, and not visible in the sample because it appends 24 records. A long-running
  consumer following the crate's own recommendation leaks. This blocks any design that checks by
  default, which is why it is here rather than filed separately: `M23.3` reached for always-on
  checking and this is what ruled it out. Decide whether completed entries are dropped, whether
  `check_quiescent` needs them, and document the answer either way.

- [ ] **M28.3** -- **Gated on `M28.1`.** Make `IoRing` generic and move the inventory inside.
  Carry the sidecar: the census found two thirds of consumers keep per-operation data beside the
  token, so an inventory that holds only tokens serves a minority. Mixed-shape consumers use a
  closed `enum` -- `tests/generated_sequences.rs` is the worked example and needs no change to
  keep working.

- [ ] **M28.4** -- **Gated on `M28.3`.** Migrate the ~12 consumers, and delete `Pending<T, X>` or
  demote it to the internal map. **Convert all of them or none**: converting a few relocates the
  duplication rather than removing it, which is the lesson `win-numa-sys` recorded the same day
  when it moved one `VirtualAllocExNuma` and left the other.

- [ ] **M28.5** -- **Answer the tokenless push.** `flush_raw` returns a bare `usize` and
  `epoch_log`'s commit path depends on it, because a flush has no buffer and a *borrowed*
  `RawHandle` gives its token nothing to guard. An inventory the ring owns has to say what it
  does with operations that have no token -- `RingContract` already models them separately with
  `observe_tokenless_push`. Note this may dissolve rather than need solving: if the sample owned
  a `SharedFile` instead of passing a `RawHandle` it could use the safe `flush` and get a token,
  which `M25.3` reopens anyway by changing how the log is opened.

- [ ] **M28.6** -- **Sweep what the break makes false**, including the README's ring examples, the
  `D-4` detail section, and every rustdoc that tells a caller to match a completion against a
  held token -- `Completion::user_data` and `IoRing::push_raw` both do, and they are the evidence
  D-55 rests on, so they are the first things the change invalidates.
