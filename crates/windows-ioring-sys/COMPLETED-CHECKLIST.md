# Completed checklist: windows-ioring-sys

Append-only. See [CHECKLIST.md](CHECKLIST.md) for pending and in-progress work.

## Moved 2026-08-22 -- M1 through M6: ring lifecycle through consumer documentation

### M1 -- Ring lifecycle and capability negotiation

- [x] **M1.1** -- Create the crate skeleton: `Cargo.toml`, `src/lib.rs`, README, and add it to the
  workspace `members`. `windows-sys` features are `Win32_Foundation`, `Win32_System_IO`, and
  `Win32_Storage_FileSystem` (which is where the IoRing bindings live). Everything is behind
  `cfg(windows)`, as elsewhere in this repository.

- [x] **M1.2** -- A typed owner for `HIORING` whose destructor is `CloseIoRing`, per the repository rule
  that a resource with a specialized destructor gets its own owner rather than a generic handle wrapper.
  Not `Clone`.

- [x] **M1.3** -- Capability query and version negotiation (D-6). `QueryIoRingCapabilities` needs no ring,
  so expose it as a free function for consumers deciding whether to use the crate at all. Ring creation
  negotiates `min(highest we understand, caps.MaxVersion)`, stores it, and exposes it. Surface
  `UM_EMULATION` rather than hiding it: a consumer reaching for this crate to maximize throughput needs to
  know the ring is emulated.

- [x] **M1.4** -- Probe every op once at construction into a cached capability set, plus
  `supports_raw(op_code)` for ops the OS has and this crate has not wrapped (D-7). The public op enum is
  `#[non_exhaustive]`.

- [x] **M1.5** -- Integration test: create and close rings at each version the machine supports, assert
  capability reporting is self-consistent (a ring never claims an op its capability set denies), and assert
  a ring created at a negotiated version reports that version back through `GetIoRingInfo`.

### M2 -- Operation identity, buffer ownership, and rundown

- [x] **M2.1** -- Duplicate `IoBuf`/`IoBufMut` from `windows-overlapped-io-sys` (D-1), with the contract
  extended to cover registration outliving a single operation. Record the extension in DESIGN-NOTES; do
  **not** extract a shared crate yet -- that decision is M6+.

- [x] **M2.2** -- `Token<B>` owning its buffer, with a generation-stamped `usize` as the `UserData` carried
  through the SQE and returned in the CQE (D-4). Validation on completion rejects a token that does not
  match the generation, so a stale token cannot claim a later operation's completion -- the same guarantee
  `OperationId` makes in `windows-overlapped-io-sys`, but cheaper here because `UserData` is ours to choose
  rather than being an address we have to stamp separately.

- [x] **M2.3** -- `Drop` on an uncompleted token forgets its buffer rather than freeing it (D-4). Test that
  the leak happens and that nothing is freed, because a use-after-free here would be silent and remote.

- [x] **M2.4** -- In-flight accounting and rundown: `CloseIoRing` must not run with operations outstanding.
  Mirror the rundown discipline `windows-overlapped-io-sys` already uses, including its bounded, rechecked
  wait rather than an unbounded one.

### M3 -- The submission builder

- [x] **M3.1** -- `Batch` holding `&mut IoRing`, submitting on drop, with `submit()` consuming it and
  returning the submitted count (D-5). Document loudly that the submission queue is ring state, so a
  dropped batch still submits -- the alternative would strand SQEs whose buffers a later unrelated submit
  would hand to the kernel.

- [x] **M3.2** -- Per-op builders reached from the batch, each returning a token from `push()`:
  `read`, `write`, `flush`, `cancel`. Options chain (`offset`, `drain_preceding` for
  `IOSQE_FLAGS_DRAIN_PRECEDING_OPS`) so the common case stays short and the barrier stays discoverable.
  Every method checks its op's capability bit from M1.4.

- [x] **M3.3** -- Submission-queue backpressure: `push()` surfaces `IORING_E_SUBMISSION_QUEUE_FULL`
  (`0x80460002`, observed at exactly entry 64 on a 64-entry queue) as a distinguishable error rather than
  auto-flushing. Auto-flush would silently change submission ordering and timing.

- [x] **M3.4** -- `submit_and_wait(n, timeout)` exposing the fused form, which is the primitive Model B is
  built on (D-3) and not merely a convenience.

- [x] **M3.5** -- The narrow unsafe raw-SQE seam (D-7), documented with the same framing as the `device`
  family's unsafe `ioctl`: it exists so a consumer is not blocked on us wrapping a new op.

- [x] **M3.6** -- Integration tests: a batch of many reads round-trips with every `UserData` preserved and
  every buffer returned; a deliberately overfilled queue reports backpressure at the right entry and stays
  usable afterwards; a dropped batch still submits; a cancel of a target that is not outstanding reports
  `ERROR_NOT_FOUND` through the completion rather than at build time.

- [x] **M3.7** -- *(added during execution)* Completion retrieval: `IoRing::try_pop` popping one
  `Completion` (identity plus result) without blocking, so a caller matches it against a held `Token` via
  `claim_if`. **Re-plan:** M3's original items assumed `Token::claim_if` alone was enough to exercise the
  M3.6 round-trip, but nothing in M1/M2 exposed a way to actually pop a *typed* completion outside of
  `IoRing`'s internal, untyped rundown drain -- M3.6 cannot be tested without this. Added and implemented
  in the same pass as M3.1-M3.6 rather than deferred, per the re-planning discipline.

### M4 -- Model A delivery: completion event and the thread pool

- [x] **M4.1** -- Refuse to construct the event-driven path when `SET_COMPLETION_EVENT` is absent, with
  `io::ErrorKind::Unsupported`, rather than silently degrading into a thread-based loop. Behavior is owned,
  not inherited: a consumer who asked for threadless delivery and got a thread has been told something
  false. The threaded path is a separate, explicit choice (M6+).

- [x] **M4.2** -- Wire `SetIoRingCompletionEvent` to a `ThreadpoolWait` from `windows-threadpool-sys`,
  using an auto-reset `WaitableHandle::event`. The drain discipline is drain-to-`S_FALSE`, re-arm, drain
  again: the spike showed the event is set when completions land and auto-resets on wait, so the worst case
  is a harmless spurious callback, but the ordering is what makes a completion arriving between the last
  pop and the re-arm impossible to lose.

- [x] **M4.3** -- Teardown: quiesce the wait, run down outstanding operations, then close the ring, in that
  order. Interaction with `CleanupGroup` documented, since a consumer will reasonably expect to put this
  in one.

- [x] **M4.4** -- Integration test: submit with `wait_operations = 0` and assert completions are delivered
  on pool threads without the submitting thread ever waiting; assert teardown with operations in flight
  neither hangs nor closes the ring early.

### M5 -- Registration

- [x] **M5.1** -- Registered file handles: a typestate carrying the registered index, so a `read` against
  a registered file cannot be written against an unregistered one by mistake.

- [x] **M5.2** -- Registered buffers, with the ownership rule that registration outlives any single
  operation (M2.1) and the pinning cost documented, because it is the axis that punishes over-sharding
  (D-8).

- [x] **M5.3** -- Integration test: a read addressing both a registered file index and a registered buffer
  index round-trips; a registration outliving many operations stays valid; dropping the registration while
  operations are in flight is refused rather than permitted.

### M6 -- Documentation for consumers, and the guidance that motivated this crate

- [x] **M6.1** -- Render the "Two delivery architectures" material from [DESIGN-NOTES.md](DESIGN-NOTES.md)
  as crate-level rustdoc and README content, not only as a maintainer-facing design note. Consumers
  reaching for this crate are trying to maximize I/O throughput; the Model A / Model B trade-off, the
  reason the NUMA node is the wrong partitioning key, and the observation that buffer placement likely
  dominates thread placement are the things they need and cannot easily derive.

- [x] **M6.2** -- A worked example of Model A: threadless delivery through the pool, which is the shape most
  consumers should start with.

- [x] **M6.3** -- Topology guidance as documentation rather than as API (D-8): how to enumerate L3 domains
  with `GetLogicalProcessorInformationEx`, why processor groups are a hard floor, and how to allocate a
  node-local pool with `VirtualAllocExNuma`. Pointers, not a partitioning policy.

## Moved 2026-08-23 -- M7: `ring-copy`, a topology-aligned sample

- [x] **M7.1** -- The `Topology -> Policy -> RingPlan` pipeline, with `Policy` as named code (`ByL3`,
  `ByNode`, `ByPackage`, `ByCore`, `Single`) rather than data. A plan names, per domain, the ring to
  create, the processors to affinitize to, and where its buffer pool should be allocated.

- [x] **M7.2** -- Reject a plan the platform cannot express, rather than emitting an impossible affinity
  mask: a fed-in description may carry one group with more than 64 processors, which is legal in the
  description and unrepresentable on Windows (topology D-10).

- [x] **M7.3** -- The copy engine itself: read and write through per-domain rings, buffers allocated with
  `VirtualAllocExNuma` and registered once per ring.

- [x] **M7.4** -- Switches for topology source (discovered, or a JSON file), policy, and **buffer
  placement** (node-local versus deliberately remote). Buffer placement is the variable most likely to
  show a real effect, because the device DMAs into the registered buffer on every operation, whereas
  callback placement is a one-time cache-warmth question.

- [x] **M7.5** -- Report per-domain throughput, and **say plainly when the host cannot show a
  difference** -- a single-node or virtualized machine will produce noise, and a benchmark that reports
  noise as a result is worse than no benchmark. The machine this was designed on reported zero NUMA nodes.

## Moved 2026-08-28 -- M11: the completion event as a ring primitive

### M11 -- The completion event as a ring primitive (external consumer proposal, 2026-08-28)

Prompted by a consumer proposal and the spike that answered it; the exchange is recorded in
[DESIGN-SESSION-2026-08-28-completion-event-multiplexing.md](design-sessions/DESIGN-SESSION-2026-08-28-completion-event-multiplexing.md),
and the decisions are [D-19](DESIGN-NOTES.md#d-19) through [D-22](DESIGN-NOTES.md#d-22).

M11.1 and M11.2 build the primitive; M11.3 then consolidates `EventDelivery` onto it *and* fixes the
stranded-backlog bug in one change. Those were separate items when the fix looked like it needed to ship
ahead of the API work; they are merged because in practice the two land minutes apart, and splitting them
would mean writing a `SetEvent`-after-arm patch that M11.3 immediately deletes.

- [x] **M11.1** -- Added `IoRing::completion_event(&mut self) -> io::Result<OwnedHandle>`
  ([D-20](DESIGN-NOTES.md#d-20)): capability-checks `IORING_FEATURE_SET_COMPLETION_EVENT`, creates and owns
  an auto-reset event, attaches it with `SetIoRingCompletionEvent`, signals it once, and returns a
  duplicate. Idempotent -- a repeat call returns another duplicate of the same event rather than attaching a
  new one, which matters because `SetIoRingCompletionEvent` *replaces* rather than adds, so a second attach
  would silently detach the first subsystem's event. The rustdoc states the
  [D-19](DESIGN-NOTES.md#d-19) contract in full: signalled on empty -> non-empty, drain to empty before
  waiting again, a wake with nothing to pop is normal. Ordering detail worth keeping: the event is stored on
  the ring *before* it is signalled, so no later failure can drop it and leave the ring signalling a closed
  (possibly recycled) handle; and because a manual `Drop` body runs before its fields drop, `CloseIoRing`
  always runs before the event handle closes. Contract tests are M11.2; a throwaway smoke check confirmed
  the setup signal, auto-reset, idempotence, and -- the one that matters for M11.3 -- that attaching to a
  ring with a completion *already* queued does signal.

- [x] **M11.2** -- Contract tests for the M11.1 primitive in
  [tests/completion_event.rs](tests/completion_event.rs), written against the *stated* rules rather than
  against current behaviour: eleven cases covering the setup signal, the backlog case (submit, let the
  completions land, *then* attach, assert the returned handle signals), idempotence, the edge itself
  (empty -> non-empty signals; a batch of eight produces exactly one wakeup; the edge re-arms after each
  drain; a full drain leaves no leftover signal), the duplicate's independence (closing one duplicate,
  and outliving the ring), the capability gate's `Unsupported` branch, and the multiplexed wait.
  Each rule is named in the test that pins it, so a failure is readable without opening
  [DESIGN-NOTES.md](DESIGN-NOTES.md).

  Every case was **verified by sabotage**, which is what makes these contract tests rather than
  behaviour snapshots -- and doing it corrected the suite three times, each correction worth keeping:

  - Suppressing `completion_event`'s setup `SetEvent` fails all eleven, the backlog case on its own
    message. That is the property M11.3 depends on.
  - Making the repeat call attach a *second* event initially failed only **one** test. The other two
    idempotence cases passed **vacuously**: asserting that the handle returned by the *second* call
    signals is satisfied perfectly well by a freshly attached event, so a silently detached first handle
    went unnoticed -- the exact bug those tests exist to exclude. Both were rewritten to assert through
    the *first* handle, and to close the *second* duplicate rather than the first; the sabotage now
    fails all three.
  - The multiplexed test's first shape proved that draining mattered but not that draining on *every*
    pass did. Sabotaging round 1's drain to a single `try_pop` fails at **round 2**, not round 3,
    because round 2's unrelated-wake drain rescues the seven stranded completions -- which is precisely
    why rule 2 says every pass and not merely every ring pass. A round 4 was added that breaks rule 2
    deliberately and asserts the resulting lost wakeup as a timeout, so the deadlock is demonstrated
    rather than described; a control run confirms the same wait returns immediately when the queue was
    drained first, so the assertion measures the edge and not an impatient timeout.

  These are integration tests because generating completions crosses the filesystem boundary, matching
  [tests/event_delivery.rs](tests/event_delivery.rs). The suite runs in about half a second, nearly all
  of it the one deliberately-asserted timeout.

- [x] **M11.3** -- `EventDelivery::new` re-expressed on top of `IoRing::completion_event`, which removed
  the second `SetIoRingCompletionEvent` call site ([D-20](DESIGN-NOTES.md#d-20)) and fixed the
  stranded-backlog bug ([D-19](DESIGN-NOTES.md#d-19)) in one change. `src/` now has exactly one
  `SetIoRingCompletionEvent` call, in `IoRing::completion_event`.

  The repro landed first and was **watched failing**: a ring handed over with eight completions already
  in its CQ delivered nothing, and
  `completions_queued_before_handover_are_still_delivered` in
  [tests/event_delivery.rs](tests/event_delivery.rs) failed on a five-second delivery timeout. After the
  consolidation it passes instantly. The two pre-existing tests in that file both hand over a *fresh*
  ring, which is exactly why the bug survived CI. `completion_event`'s signal-once-on-attach is what
  makes it pass; `ThreadpoolWait` takes the returned duplicate through
  `WaitableHandle::assume_waitable`, and the drain/re-arm/drain callback body needed no change.

  Also removed: `EventDelivery`'s own copy of the capability check and its `Unsupported` error message,
  which duplicated `completion_event`'s word for word. One statement of that rule now, not two.

  Documentation corrected in the same commit, per CONTRACT INTEGRITY's blast-radius rule. The rustdoc
  had asserted the backlog guarantee while the code did not hold it, so it is now stated as a guarantee
  the method *buys* -- naming the edge that would otherwise strand the backlog and the setup signal that
  closes it -- with a note that a caller on an earlier version cannot rely on it. Swept
  `SetIoRingCompletionEvent` / "already queued" / "handed over" across `src/`, `tests/`, `examples/` and
  `*.md`: 6 files updated ([src/event_delivery.rs](src/event_delivery.rs),
  [src/capability.rs](src/capability.rs), [examples/model_a_delivery.rs](examples/model_a_delivery.rs),
  [DESIGN-NOTES.md](DESIGN-NOTES.md)'s D-20 status marker, its Model A description, and its "this bit us"
  paragraph). Design-session transcripts and [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) were left
  alone as historical record.

  One newly reachable interaction documented on `EventDelivery::ring`: calling
  `IoRing::completion_event` on a ring the pool is already waiting on now *succeeds*, returning a
  duplicate of the pool's own event, which is two waiters on one ring and a
  [D-21](DESIGN-NOTES.md#d-21) violation. Worth stating because the same call was previously worse and
  equally silent -- it replaced the pool's event and stopped delivery outright.

- [x] **M11.4** -- `windows-threadpool-sys` is now optional behind a default-on `threadpool` feature
  ([D-22](DESIGN-NOTES.md#d-22)). Gated with it: the `event_delivery` module and its unit tests, the
  `EventDelivery` re-export, [tests/event_delivery.rs](tests/event_delivery.rs), and
  [examples/model_a_delivery.rs](examples/model_a_delivery.rs) -- the example via an explicit
  `[[example]]` with `required-features`, so cargo *skips* it rather than failing to compile it.
  Verified non-vacuous by `cargo tree`: `windows-threadpool-sys` disappears from the graph entirely
  under `--no-default-features`, and the other two examples still auto-discover and build.

  CI gained an `ioring-no-threadpool` job, which is the cost D-22 accepted the gate with: nothing else
  in the workflow builds that configuration, because every `--workspace` step uses the default set and
  `--all-features` turns the gate back on. It runs build, clippy `-D warnings`, test, and doc for
  `--no-default-features`, plus isolated default-feature clippy and test steps (selecting the crate
  alone, so `--workspace` feature unification cannot mask a gap -- the same reason
  `windows-file-watcher` has isolated steps).

  The `cargo doc` step is there because of a defect this item found rather than by symmetry: the
  crate's ungated "Choosing a delivery architecture" prose intra-doc-linked ``[`EventDelivery`]``,
  which resolves under `--all-features` and *dangles* without the feature. The repo-wide `docs` job
  only documents `--all-features`, so it could never have caught it. The link is now plain code naming
  the feature, matching how `windows-overlapped-io-sys` avoids linking its own gated items from ungated
  prose; the failure was confirmed by restoring the link and watching
  `cargo doc --no-default-features` fail with `unresolved link to EventDelivery`.

  Also documented for consumers: a Cargo-features table in [README.md](README.md) with the
  `default-features = false` snippet and D-22's actual rationale -- layering, not runtime cost, since
  linking the thread-pool crate creates no threads (the Win32 default pool is process-wide and lazily
  instantiated).

- [x] **M11.5** -- Both facts now stated wherever the wakeup shapes are described, as a blast-radius
  sweep rather than a single edit. Grepped `drain_preceding` / `DRAIN_PRECEDING` / "two delivery" /
  "completion event" / "delivery architecture" across `src/`, `tests/`, `examples/` and `*.md`; 18 files
  matched, 4 changed, and the rest are accounted for below.

  **Fact 1 -- Model B's wakeup source is separable from its identity.** The
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "Two delivery architectures" section described Model B as a thread
  parked in `SubmitIoRing` and offered no alternative, which is exactly how the framing came to be read
  as fixing the wakeup mechanism ([D-3](DESIGN-NOTES.md#d-3)'s amendment note). It gained a subsection
  naming the two wakeup sources -- fused submit-and-wait, and a multiplexed `WaitForMultipleObjects` over
  `IoRing::completion_event` -- as a table, with the point that identity is *who owns, submits and
  drains* and the wakeup is a separate axis. [README.md](README.md) gained the same in consumer form.
  [src/lib.rs](src/lib.rs) already stated it (M11.1); its Model B paragraph gained a forward pointer so a
  reader who stops there does not leave with the fixed-wakeup reading.

  **Fact 2 -- `drain_preceding`'s barrier stops at the ring's edge.** Stated in
  [DESIGN-NOTES.md](DESIGN-NOTES.md)'s Category 2 and in [src/lib.rs](src/lib.rs), but **not on
  `PushOptions::drain_preceding` itself** -- the one place a consumer actually meets the flag. That was
  the real gap this sweep found. Its rustdoc now states all three measured properties: ring-wide and
  spanning submissions with no cross-epoch pipelining ([D-24](DESIGN-NOTES.md#d-24)), powerless in both
  directions across the ring boundary with `IoRing::completion_event` as what this crate offers instead,
  and that the flag orders but does not flush -- while being what makes a flush cover preceding writes
  at all ([D-23](DESIGN-NOTES.md#d-23)). README states the limit too, as the reason the multiplexed
  shape has to exist.

  **Accounted for, unchanged:** `IoRing::completion_event` and the `IoRing` type docs in
  [src/ring.rs](src/ring.rs) already state both facts correctly (M11.1);
  [src/event_delivery.rs](src/event_delivery.rs) and
  [examples/model_a_delivery.rs](examples/model_a_delivery.rs) are Model A only and make no wakeup claim
  beyond it; [src/batch/tests.rs](src/batch/tests.rs) and the integration tests are uses of the flag and
  the primitive rather than statements about them (the cross-path limit is not testable in-crate -- it
  is a fact about I/O this crate does not issue); [PLANS.md](PLANS.md) is an index summary with no
  wakeup claim; and [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), the design-session transcripts and
  the spikes are historical record, deliberately left alone.

- [x] **M11.6** -- [examples/model_b_multiplexed.rs](examples/model_b_multiplexed.rs): a caller-owned ring
  whose `completion_event` is waited on via `WaitForMultipleObjects` alongside a manual-reset shutdown
  latch, draining to empty on every pass, over three waves of reads. It also covers the half the item did
  not name but which a consumer hits immediately -- shutdown while I/O is outstanding -- by quiescing
  through `Batch::submit_and_wait` before the ring closes, which doubles as a demonstration that both
  wakeup sources are Model B (M11.5's fact 1) and that switching between them changes nothing about
  ownership.

  **The central claim was verified by sabotage.** Replacing the drain-to-empty with a single `try_pop`
  reproduces the lost-wakeup deadlock exactly: wave 0 pops one of eight completions, the queue never
  returns to empty, the edge never re-arms, and the wait times out with all remaining work stranded. That
  is [D-19](DESIGN-NOTES.md#d-19) as a running program rather than a paragraph, which is the whole point
  of the item.

  Two comments were **removed after measurement contradicted them**, rather than left as plausible
  prose. The example requests shutdown while the final wave is outstanding, and an earlier draft claimed
  that made the drain-on-shutdown-pass and the quiesce load-bearing. Six runs showed otherwise: the loop
  always exits with zero in flight, because `WaitForMultipleObjects` reports the *lowest* signalled index
  and the ring is index 0, so a set shutdown latch can never starve completions. That is a genuinely
  useful fact for a consumer choosing handle order, so the example now states it and reports what
  actually happened on each run instead of asserting an outcome it does not control.

  The example uses no thread pool, so it is also the first real consumer of the `--no-default-features`
  configuration M11.4 added to CI; verified building under both feature sets.

## Moved 2026-08-28 -- M12: durability exposed, and the flush barrier made explicit

### M12 -- Durability: expose it, and stop defaulting it wrong

The kernel exposes a durability parameter on writes (`FILE_WRITE_FLAGS`) and on flushes
(`FILE_FLUSH_MODE`); this crate hardcodes both, so a consumer sees ordering but no way to express
durability at all. Worse, [D-23](DESIGN-NOTES.md#d-23) measured that an unflagged flush does *not*
cover preceding writes -- which made `Batch::flush(&file, PushOptions::default())`, the obvious
spelling, a silent data-loss bug rather than a missing feature. **M12.1 removed that spelling**; the
remaining items expose the parameters the crate still hardcodes.

Decisions: [D-23](DESIGN-NOTES.md#d-23) through [D-25](DESIGN-NOTES.md#d-25). Measurements are
reproduced by the drain spike recorded in
[DESIGN-SESSION-2026-08-28-external-consumer-correspondence.md](design-sessions/DESIGN-SESSION-2026-08-28-external-consumer-correspondence.md).

M12.1 is first because it is a correctness defect in shipped 0.1.2, not an enhancement.

- [x] **M12.1** -- The barrier decision is now explicit for flushes ([D-23](DESIGN-NOTES.md#d-23),
  [D-25](DESIGN-NOTES.md#d-25)). `Batch::flush` and `Batch::flush_raw` take a required
  `FlushCoverage` -- `CoversPrecedingOperations` or `Unordered` -- **in place of** `PushOptions`,
  rather than alongside it. Replacing it rather than adding to it is the point: keeping both would let
  a caller write `FlushCoverage::Unordered` with `PushOptions::new().drain_preceding(true)` and mean
  two contradictory things at once. `PushOptions` carries exactly one decision, and for a flush
  `FlushCoverage` *is* that decision.

  An enum rather than a `bool` because `flush(&file, true)` does not say what the `true` decides, and
  a two-variant type makes the wrong choice unwriteable by accident rather than merely discouraged.
  The `Unordered` variant is documented with the two uses that are legitimate (host sequencing, and a
  flush not being used for durability at all) so it reads as a deliberate choice rather than an
  escape hatch.

  Rustdoc on `flush_raw` now states the measured contract in full: that the ring has no FUA and the
  flush is therefore its only durability primitive, that an unflagged flush was measured completing
  while 17 and then 23 of 32 preceding writes were still outstanding, and that durability is a
  property of an epoch rather than of a write. `PushOptions::drain_preceding` cross-references it so a
  reader arriving from the flag side learns the flush no longer inherits the decision.

  A unit test pins the `FlushCoverage` -> SQE-flag mapping, **verified by sabotage**: making
  `CoversPrecedingOperations` map to `IOSQE_FLAGS_NONE` compiles and passes every other test in the
  crate, and would lose data only on power failure -- exactly the class of defect that needs a
  mechanical check rather than review.

  Swept the statements this changed: [DESIGN-NOTES.md](DESIGN-NOTES.md)'s epoch construction and
  barrier-cost table now name the API instead of the raw flag, [D-25](DESIGN-NOTES.md#d-25) carries an
  implementation-status marker, and the M12 intro above no longer describes a spelling that exists.
  The two `flush_raw` call sites in [tests/submission_lifecycle.rs](tests/submission_lifecycle.rs) use
  `Unordered` with a note saying why (they test backpressure, not durability). The broader durability
  sweep across `lib.rs` and `README.md` stays M12.5, which is where those documents first have to
  discuss durability at all.

  **Breaking change**: both flush entry points changed signature.

- [x] **M12.2** -- [tests/flush_barrier.rs](tests/flush_barrier.rs) proves the barrier *behaviour*,
  not the flag. The unit test in [src/batch/tests.rs](src/batch/tests.rs) pins the enum-to-SQE-flag
  mapping, which shows the flag is set but not that setting it changes what the kernel does; this
  reproduces the D-23 shape against a real device and asserts the difference. Verified by sabotage --
  mapping `CoversPrecedingOperations` to `IOSQE_FLAGS_NONE` fails it on behaviour, not on a flag
  comparison.

  All three of the spike's failed iterations are encoded as requirements rather than left to be
  rediscovered: `FILE_FLAG_NO_BUFFERING` (buffered writes finish in issue order), a pre-written extent
  (extending writes serialize), and a **size asymmetry** between the two phases (uniform sizes do not
  reorder). The third was not in the item's text and cost a full rewrite to find: a first version used
  32 uniform writes and one flush, and skipped on this machine because it could observe nothing.

  **The measurement disagreed with D-23, and the disagreement is now recorded rather than smoothed
  over.** On this machine *no* preceding write ever completed after an unflagged flush -- 0 of 32,
  against the spike's 17 and 23 -- yet 11 of 32 writes queued *after* the flush completed before it.
  Reordering was plainly happening; it just did not manifest as the flush overtaking the writes ahead
  of it. So the control accepts *either* direction of reordering and skips only when it sees neither;
  requiring D-23's specific observable would have made the test silently vacuous here.
  [D-23](DESIGN-NOTES.md#d-23) now carries the amendment, and "The two measured facts" states the
  consumer-facing consequence: **observing that your flush lands last is not evidence you can omit the
  barrier** -- it is incidental behavior of one device stack, which is exactly what PLATFORM INTEGRITY
  says never to bind to.

  The covering case also asserts [D-24](DESIGN-NOTES.md#d-24)'s other half (writes queued after a
  drained flush are held until it completes), which is the observable that actually discriminates on
  this hardware. Short-write detection guards the whole thing: an unbuffered write with a misaligned
  offset or length would otherwise make every count meaningless.

- [x] **M12.3** -- `FILE_WRITE_FLAGS` is exposed as `WriteCaching` (`Cached` / `WriteThrough`), a
  required argument on all four write entry points -- `write`, `write_raw`, `write_registered`,
  `write_registered_raw` ([D-25](DESIGN-NOTES.md#d-25)). A typed enum rather than a raw flag word, and
  `Cached` is `#[default]` so the previous hardcoded behaviour has a name rather than being the
  absence of one.

  The rustdoc states what write-through **is** -- a first-level cache directive whose value is
  latency shaping, since data already at the device makes a later flush shorter -- and what it is
  **not**: not a durability guarantee and not FUA, because whether it becomes a Force Unit Access bit
  depends on the driver, the volume, and whether the device's write cache is enabled, none of which
  this API can see or promise. A write that completes with `WriteThrough` may still be in a volatile
  device cache. That conflation cost the originating exchange a wrong recommendation, so it is stated
  at the type rather than left in a design note.

  A unit test pins the mapping, including that `Cached` stays the no-flag value: silently enabling
  write-through would change latency behaviour for every existing caller without changing any call
  site.

- [x] **M12.4** -- `FILE_FLUSH_MODE` is exposed as `FlushMode` (`Default`, `Data`, `MinMetadata`,
  `NoSync`) on both flush entry points ([D-25](DESIGN-NOTES.md#d-25)), with `Default` as `#[default]`
  -- the mode a durability barrier wants and what the crate hardcoded before.

  `NoSync` carries the loudest documentation in the crate, because it is **the one mode that makes
  nothing durable**: it pushes data out of the system cache and stops, so anything in a volatile
  device cache is lost on power failure exactly as if no flush had been issued. Its completion is not
  a commit point and must never be reported to a caller as one. Stated in passing, as the item asked:
  the existence of a distinct "no sync" mode is itself the evidence that the other three *do* issue
  the sync -- nothing in the Win32 documentation says so directly.

  A unit test pins all four mappings. It matters most for `NoSync`: confusing it with any other value
  would turn a commit point into a no-op that still reports success.

  **Note on sequencing.** These two were implemented back to back and are committed together, citing
  both IDs. They are genuinely independent -- separate kernel parameters on separate entry points --
  and that independence was preserved in how the code was written, so re-slicing the commit would be
  bookkeeping rather than history.

  **Breaking change**: all four write entry points and both flush entry points changed signature.

- [x] **M12.5** -- Durability is now stated wherever it is discussed, as a blast-radius sweep.
  Grepped `flush` / `durab` / `write_through` / `WriteThrough` / `drain_preceding` / `FUA` across
  `src/`, `tests/`, `examples/` and `*.md`.

  **The finding was an absence, not a contradiction.** [src/lib.rs](src/lib.rs) and
  [README.md](README.md) between them mentioned durability *zero times* -- one incidental hit each, both
  the word "flush" in the list of the kernel's seven ops. A consumer reading either document
  front-to-back would have learned that the ring exists, how to choose a delivery architecture, and
  how to size an execution domain, without ever being told that a flush is the only way to commit
  anything or that the obvious spelling of one commits nothing. Both now carry a `Durability` section
  stating the three facts the item names -- no FUA, the flush is the only durability primitive, a
  flush without the barrier covers nothing -- plus the consequence that ties them together: durability
  is a property of an epoch, never of an individual write. README's third point also carries M12.2's
  measurement, that seeing your flush land last is device-dependent and not evidence the barrier can
  be omitted.

  **Accounted for, already correct:** the flush and write rustdoc state all three facts in full
  (M12.1, M12.3, M12.4), which is where the summaries point; `PushOptions::drain_preceding` states the
  barrier's reach and its relationship to durability (M11.5);
  [DESIGN-NOTES.md](DESIGN-NOTES.md)'s "Durability on the ring" is the long form all of them defer to.
  [tests/flush_barrier.rs](tests/flush_barrier.rs) and [src/batch/tests.rs](src/batch/tests.rs) are
  checks of the contract rather than statements of it; the `ring_copy` example's writes are a copy
  pipeline with no durability claim to make, and now name `WriteCaching::Cached` explicitly rather
  than inheriting a hardcoded flag.
