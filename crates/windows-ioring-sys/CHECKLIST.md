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


## M26+ -- The wakeup window review opened

- [ ] **M26.12** -- **Find why a signal raised just after `wait.arm` can be lost, and fix it.**
  Raised as a narrower finding by Copilot review on PR #108 -- that `EventDelivery::new` signals
  only when it attached the event itself -- and the investigation found something wider.

  **What is measured.** The `#[ignore]`d reproducer
  `a_backlog_is_delivered_even_when_the_caller_attached_the_event_first` in
  [event_delivery.rs](tests/event_delivery.rs) fails 6 of 6. Signalling unconditionally, which is
  what the review suggested, does not change that. A 50 ms sleep between `wait.arm` and the signal
  makes it pass 3 of 3, and so does `--features trace`, which is the same perturbation by another
  route. Full figures in [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md).

  **Why this is not a small follow-up.** [D-68](DESIGN-NOTES.md#d-68) fixed `M26.9`'s stall by
  ordering the arm before the signal, measured at 0 failures in 3600 runs. This says that ordering
  narrows the window rather than closing it, so the decision's reasoning needs revisiting once the
  mechanism is known -- not before, because the mechanism is currently a guess.

  **Do not apply the sleep.** It is a diagnostic that identified a window, not a fix, and shipping
  it would convert a reproducible defect into a rare one.
## M28+ -- Opened by the inventory

- [ ] **M28.7** -- **Decide whether the ring should check conservation itself, rather than a
  caller driving `RingContract`.** Raised by [D-74](DESIGN-NOTES.md#d-74) and deliberately not
  taken there. Once the inventory is the only push path, a pop already knows whether the identity
  was stowed, and `held()`/`outstanding()` are both the ring's own numbers -- so
  `UnexpectedCompletion`, `DuplicateCompletion` and `Outstanding` are all answerable without a
  caller reporting anything.

  **Why it is a decision and not a cleanup.** [pending.rs](src/pending.rs) recorded the structural
  complaint that an oracle's "value depends on being driven correctly by the very code it checks",
  and this would answer it. But `RingContract` is deliberately *not* wired into `Batch` ([its own
  rustdoc](src/contract.rs) says why): a ring driven through `push_raw` bypasses this crate's
  bookkeeping entirely, so an internal hook would cover less than it appears to, and a consumer
  validating its own harness needs to drive the same rules from outside. Moving the checking
  inward trades that away. Gated on `M28.4.1d`.
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

- [x] **M28.1** -- Decided: a push returns a `Copy` identity that owns nothing, and the ring returns the payload at its own pop -- `Token` is split, not moved. Recorded as [D-71](DESIGN-NOTES.md#d-71). -> [completed 2026-09-25](COMPLETED-CHECKLIST.md#m281)

- [x] **M28.2** -- `RingContract` is bounded by operations in flight: terminal entries are retired, and a capped history keeps a duplicate distinguishable from an unrecognised completion. Recorded as [D-72](DESIGN-NOTES.md#d-72). -> [completed 2026-09-25](COMPLETED-CHECKLIST.md#m282)

- [ ] **M28.3+M28.4** -- **Make `IoRing` generic, move the inventory inside, and migrate every
  consumer, as one commit.** Gated on [D-71](DESIGN-NOTES.md#d-71), which settled what a caller
  receives.

  **Merged deliberately, and the coupling is acknowledged rather than disguised.** The milestone
  header requires each step to compile, `M28.4` requires converting all consumers or none, and
  `M28.3` changes `IoRing`'s shape -- so the three cannot all hold with the items separate. The
  alternative considered and rejected was landing `M28.3` additively, with an `OperationId` path
  beside the existing `Token` one: it compiles at every step, but it leaves two token models live
  at once and the old one still lets a consumer lose a token, which is the defect `D-55` exists to
  remove. Never having both is worth one large commit.

  **Carry the sidecar.** The census found two thirds of consumers keep per-operation data beside
  the token, so an inventory holding only tokens serves a minority. Mixed-shape consumers use a
  closed `enum`; [generated_sequences.rs](tests/generated_sequences.rs) is the worked example.

  **Soundness already settled:** [`IoBuf`](src/buf.rs) is an unsafe trait whose contract requires
  the address to survive a move, so the ring may hold buffers in a map.

  Sequenced so the work is resumable, since it does not compile in the middle:

  - [x] **M28.3.1** -- `OperationId`: `Copy`, no `Drop`, a name and not a capability (`D-71`).
  - [x] **M28.3.2** -- `IoRing<T = ()>` carrying `inventory: HashMap<usize, T>`, and
        `Batch<'ring, T>` with it. A default keeps a payload-free consumer from naming `()`.
  - [x] **M28.3.3** -- Push stores the payload and returns an `OperationId`; pop returns the
        payload with the completion. Shape settled by [D-73](DESIGN-NOTES.md#d-73):
        `IoRing<T, X = ()>`, with the file guard held in a concrete internal `Held` rather than
        made generic, which is sound because `FileTarget` is sealed.

        **Do the `Drop` half in this step, not later.** Moving buffers into the ring removes the
        protection `Token`'s leak-on-unclaimed-drop provides on the path where `run_down` fails --
        which `Drop for IoRing` takes best-effort, closing anyway. The inventory must be
        *forgotten* rather than dropped there, or the close frees memory the kernel may still be
        writing into. Landing the inventory without this is a use-after-free, so it is one step.

        The tokenless shape is `M28.5`'s and is only accommodated here, not answered.
  - [x] **M28.3.4** -- Carry the parameter through `EventDelivery`, `RingScope` and the contract
        wiring.

        **The tree stops compiling here, as this plan said it would.** The delivery callback is
        now `Fn(Completion, Option<(T, X)>)`, so ten call sites across
        [event_delivery.rs](tests/event_delivery.rs), [handover.rs](tests/handover.rs),
        [checkpoint.rs](examples/epoch_log/checkpoint.rs) and
        [model_a_delivery.rs](examples/model_a_delivery.rs) take a one-argument closure and no
        longer build. The library and its own unit tests are green; the integration and example
        targets are `M28.4.1`'s to migrate. Contract wiring is untouched deliberately -- what
        becomes of the oracle's leak rules is a decision `M28.4.1` carries, per
        [D-73](DESIGN-NOTES.md#d-73).
  - [x] **M28.4.1a** -- Restore the tree: every delivery callback takes the payload, so the
        build is green again on every feature set.

        **Two call sites were invisible to an ordinary sweep**, and both are worth remembering
        rather than rediscovering. One lives in a **doctest**, found only because this repository
        compiles its prose. The other is in [failure_paths.rs](tests/failure_paths.rs), which
        compiles only under `--all-features`, so a default-feature check could not see it. A
        migration sweep here has to run `--all-features` **and** `--doc` before it means anything.

  - [x] **M28.4.1b** -- All ten push shapes have an inventory form: `read_raw_owned`,
        `write_raw_owned`, `read_owned`, `write_owned`, `flush_owned`, `cancel_owned`, and the
        four registered variants. `Held` is populated by the guarded pushes and
        `Held.registration` by the registered ones, so both halves are exercised and neither
        needs a dead-code marker any longer. `read_owned` is the only
        entry point that stows, so "migrate the consumers" has no destination for the other ten
        shapes yet -- writes, flushes, cancels, and the registered variants. Give each an
        inventory form first, populating `Held` for the guarded ones, which is what retires the
        `#[expect(dead_code)]` on `Held` and `FileGuard`.

  - [x] **M28.4.1c** -- Decided in [D-74](DESIGN-NOTES.md#d-74): the leak rules **retire** rather
        than narrow, because a push that hands back only an `OperationId` leaves nothing to drop
        unclaimed. `State::Pushed` and `State::PushedTokenless` collapse with them. The
        conservation they approximated becomes `held() == outstanding()`, which the ring can check
        about itself. `RingContract` keeps the four claims that are about the kernel rather than
        about a caller's bookkeeping.

  - [ ] **M28.4.1d** -- Migrate every consumer onto the inventory, then retire the token API.

        **Measured before planning, and it is larger than "36 files" suggested**: 172 push call
        sites and **116 `claim_if` sites** across 35 files. `claim_if` is not a substitution --
        it is how each test *drives* its ring, so converting restructures control flow rather
        than replacing a call.

        **What a conversion actually does, which is why it is worth it.** The caller's
        `HashMap<usize, (sidecar, Token<..>)>` *disappears* at each site: the push carries the
        sidecar as `X`, and the pop returns `(payload, sidecar)` together. That is `D-55` paying
        off rather than a cost being paid.

        **Batched, and the reason that is legitimate.** Both APIs coexist today, so a
        partly-converted tree still compiles and every batch is a green commit. `M28.4.2`'s
        "convert all of them or none" governs the **shipped** state -- never two token models in
        a release -- not the path to it. The final batch is what makes that true, and nothing is
        released in between.

  - [x] **M28.4.1d.1** -- [bounded_pop.rs](tests/bounded_pop.rs) converted as the worked
        pattern. The `Token<Vec<u8>>` threaded through `push_pending_read`, `settle` and six call
        sites is gone; `PipeRing = IoRing<Vec<u8>>` holds the buffer instead. The conversion
        found a real defect in rundown, which is recorded as `M28.4.1d.1b`.

  - [x] **M28.4.1d.1b** -- Decided: there should be one pop, not two. **Decide what `try_pop`
        and `pop_within` mean on a ring that holds payloads.** Found by converting the first file: `drain_for_rundown` popped with `try_pop`
        and never reclaimed, so rundown stranded every entry it reaped. Fixed there -- rundown is
        teardown, so dropping is right, and the completion is the proof that makes freeing safe.

        **The same gap is open on the public paths, and the answer is that the split should not
        exist.** An earlier note here offered three ways to manage a permanent distinction
        between `try_pop` and `try_pop_held`. That was wrong, and the symmetry is the argument:
        `try_pop_held` sits beside `try_pop` for exactly the reason `read_raw_owned` sits beside
        `read_raw` -- the inventory was added *beside* the token API rather than replacing it.
        Both are the same transitional duplication, and the push side is already settled as
        ending with one family.

        **Nothing needs a non-reclaiming pop.** Measured: the only internal callers are
        `drain_for_rundown` (now reclaims), `pop_within_with`, and one other -- and once the
        token pushes are gone, *every* operation has an inventory entry, including the tokenless
        ones, which stow `payload: None`. There is no operation a pop could legitimately find
        nothing for.

        So `try_pop` becomes the reclaiming pop, `try_pop_held` disappears as a name, and
        `pop_within` returns the same shape. Preserving the distinction would be a rule against
        an impossible act, which [D-74](DESIGN-NOTES.md#d-74) already names as worse than no rule
        at all.

        **This does not gate the conversion.** Consumers migrate onto the reclaiming pop either
        way; the rename lands in `M28.4.1d.3` beside the push retirement.

  - [x] **M28.4.1d.2** -- Every consumer that can be converted before the token API is retired now is; three plus `append.rs` are blocked on `M28.4.1d.3`. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d2)

  - [x] **M28.4.1d.2b** -- Documented the two ways round a mixed payload on `IoRing::with_inventory` and in `D-73`; no `IoBuf` enum helper built. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d2b)

  - [x] **M28.4.1d.3** -- Token API retired, `D-74` applied, one reclaiming pop per shape; the break is closed. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d3)

  - [ ] **M28.4.2** -- Sabotage the inventory: a push that does not record, and a pop that does
        not retire, must both turn the suite red.

        The pop half now has a worked precedent to follow rather than invent: `M28.4.1d.2`
        made both public pops retire their entry, and verified it by re-injecting the
        stranding -- five `registration.rs` tests went red, and all eighteen passed once it
        was restored. Record that as a case in `sabotage.json` rather than leaving it as a
        command that was run once and discarded.

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