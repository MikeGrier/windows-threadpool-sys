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

## Moved 2026-08-29 -- M13: the epoch-committed log worked example

### M13 -- Worked example: consumer-side durability (an epoch-committed log)

[D-26](DESIGN-NOTES.md#d-26) puts durability *policy* with the consumer and Windows *mechanism*
here, which leaves a gap: without a demonstration, every consumer rediscovers the same composition,
and the three measured contracts ([D-19](DESIGN-NOTES.md#d-19), [D-23](DESIGN-NOTES.md#d-23),
[D-24](DESIGN-NOTES.md#d-24)) are exactly the kind that are learned by deadlock or by data loss.
This milestone closes that gap with a worked example, not a library -- it demonstrates the pattern
without this crate owning the policy.

The example is a miniature write-ahead log: records appended through the ring, made durable by
group commit, with durability reported by epoch. It is deliberately the shape a real consumer
needs, and it exercises `windows-ioring-sys` and `windows-threadpool-sys` together.

**Depends on M11.1** (`IoRing::completion_event`) and **M12.1** (explicit flush barrier). Both have
landed, so this milestone is unblocked.

- [x] **M13.1** -- Scaffolding under [examples/epoch_log/](examples/epoch_log/) --
  [main.rs](examples/epoch_log/main.rs) and [contract.rs](examples/epoch_log/contract.rs) -- with the
  sample's own durability contract written down first, before any code that implements it.

  The contract is phrased as *this program's specification*, not as a description of what the ring
  happens to do, which is the Design Autonomy rule applied rather than cited: the mechanisms it picks
  (a covering flush, the ring's completion event) are recorded as chosen *because* they satisfy the
  specification, and the file states plainly that a dependency which stops satisfying it is the thing
  that is wrong. The guarantee is the item's sentence -- **a record is durable when the commit of the
  epoch containing it has completed** -- with each of its four load-bearing phrases unpacked, since
  "has completed" doing the work of "was submitted" is exactly how this contract gets misread.

  All three non-guarantees the item names are stated as plainly as the guarantee (no per-record
  durability, no ordering within an epoch, no atomicity past the device's power-fail atomic write
  unit), plus two the writing surfaced: nothing is promised about records after the last committed
  epoch -- present, absent, and torn are all legal outcomes of one crash -- and the whole contract
  rests on the device honoring the flush, which nothing here can verify. Those are recorded as an
  explicit `Assumes` clause rather than left silent.

  The contract is also machine-readable (`CONTRACT: &[Statement]`, grouped by `Clause`) and the
  sample prints it, so a reader who only runs the program still learns what it does and does not
  promise. M13.5's verification pass has something concrete to refer back to.

- [x] **M13.2** -- The append path, in [record.rs](examples/epoch_log/record.rs) (the format) and
  [append.rs](examples/epoch_log/append.rs) (the arena and the push). A record is a 20-byte header --
  magic, sequence, payload length, checksum -- followed by its payload, with each field justified by
  a clause of the contract rather than by convention: the length because replay has no framing of its
  own, the sequence because the contract guarantees no ordering *within* an epoch so the on-disk
  order is not the logical one, and the checksum because the contract says the tail may be **torn**
  and a reader that cannot tell torn from whole cannot honour that clause. The checksum covers the
  header too, so a torn header carrying a plausible length is caught rather than trusted.

  Appends compose into a registered arena (`SLOTS` slots) and push with `write_registered_raw` over
  exactly the record's bytes, deliberately not the owned-`Vec` form: an externally-managed arena is
  what a real consumer has, and it is what makes this sample worth reading. `PushOptions::new()` and
  `WriteCaching::Cached` are both the *unordered, uncached* choice on purpose -- records stream
  unordered within an epoch exactly as the contract says, and the ordering is bought once by M13.3's
  covering flush. The example runs the arena dry on purpose (24 records through 8 slots), so the
  `WouldBlock`-then-drain path is exercised rather than hypothetical.

  **This item found a real gap in the crate and fixed it at the layer, per the mono-repo policy.**
  `RegisteredBuffers` exposed `get` but no mutable accessor, so a registered arena could only ever
  carry bytes the *kernel* had produced -- there was no way to put a record a caller composed into
  one. `ring_copy` never hit it because it reads into a registered buffer and writes back out of the
  same one. Raised rather than worked around, and the engineer chose per-buffer accounting over a
  narrow `unsafe` seam.

  The subtlety that made it more than an accessor: **`&mut self` is not sufficient for safety here.**
  An in-flight operation holds no borrow -- `write_registered` takes `&RegisteredBuffers` for the
  length of the call and the `Token` keeps only a `RegisteredUse` -- so the borrow checker would
  happily allow mutating a buffer the kernel is reading through. `get_mut` therefore pairs `&mut
  self` with a runtime check, and the outstanding count moved from one per *registration* to one per
  *buffer* so that a busy slot does not block its neighbours, which an arena being refilled needs.
  `WouldBlock` and `InvalidInput` are distinct so "busy" and "no such buffer" cannot be confused.
  Sabotage-verified: reverting `get_mut` to the old per-registration semantics fails the new
  neighbour test with "buffer 1 still has 1 operation(s) outstanding".

  Two tests in [tests/registration.rs](tests/registration.rs) cover it -- filling a registered buffer
  and writing it back out, and the refusal/neighbour/out-of-range/release cycle. The example also
  reads its own log back and decodes every record, so the claimed format is checked rather than
  asserted; verifying the *contract* (durability, the torn tail) stays M13.5's job.

- [x] **M13.3** -- Epoch bookkeeping and group commit in [commit.rs](examples/epoch_log/commit.rs).
  Records join whatever epoch is open, closing epoch *N* pushes **one covering flush** whose
  `UserData` is *N*'s identity for the rest of its life, and observing that flush's completion is what
  advances `durable_through`. The item's completion test -- a caller can await "epoch *N* is durable"
  and get a truthful answer -- is `Committer::is_durable`, and the example awaits each epoch before
  moving on.

  **Truthfulness is the whole item, so it is asserted rather than assumed.** The example checks
  `!is_durable(closed)` in the window between pushing a commit and observing it, checks monotonicity
  across every epoch below the watermark, and checks that the still-open epoch never reports durable.
  Sabotage-verified: advancing `durable_through` at push time instead of at completion fails on "a
  pushed commit is not a completed one". A failed commit advances nothing, which is the truthful
  answer for the epoch it was closing.

  Two things the writing forced, both recorded in the module rather than left implicit. The barrier is
  **ring-wide**, so a commit stalls the log -- next-epoch appends queue behind it and hold their arena
  slots -- which is D-24's cost made visible rather than described. And because it is ring-wide, a
  commit of *N* may in fact also cover records already pushed into *N+1*; the committer deliberately
  **reports less than is true**, advancing only to *N*, because reporting less than reality is always
  safe and reporting more never is.

  **The record format gained an `epoch` field**, extending M13.2's header from 20 to 28 bytes. Keeping
  the record-to-epoch mapping only in RAM would have made the contract verifiable in this
  demonstration and unverifiable in the situation it exists for: a replay after a crash runs in a new
  process with no memory of which record joined which epoch. The read-back check now asserts every
  record carries the epoch that was open when it was appended.

  The blocking wait here is the fused submit-and-wait; M13.4 replaces it with the multiplexed one,
  which changes the shape but not the accounting.

- [x] **M13.4** -- The event loop in [event_loop.rs](examples/epoch_log/event_loop.rs): the ring's
  `completion_event` waited on by `WaitForMultipleObjects` alongside a manual-reset shutdown latch,
  with the drain outside the match on which handle woke us and draining *to empty*. Ownership does
  not change -- this thread still owns, submits to, and drains its ring -- so it is Model B with a
  different wakeup source, not Model A. The fused wait M13.3 used is kept beside it as the documented
  contrast rather than deleted, since it is the right choice for a log whose only I/O is ring I/O.

  **The item asked for the drain-to-empty rule to be visually obvious, and sabotage corrected which
  half of it actually matters.** Replacing drain-to-empty with a single `try_pop` deadlocks the
  example outright: the queue stops returning to empty, the edge never re-arms, and the next wait
  blocks until its 30-second timeout with the log's work stranded. That half has teeth and is now
  demonstrated.

  Moving the drain *inside* the ring's arm, by contrast, **does not break a conformant loop** -- the
  sabotage passed. On a loop that already obeys rule 1 the shutdown pass has nothing to pop, because
  any completion that arrived signalled the event and was drained on the ring's own wake. An earlier
  draft claimed the unconditional placement was "the bug this file exists to demonstrate the absence
  of"; that claim was false and is removed. The placement stays because rule 1 says every pass and it
  costs nothing, and the file now says plainly that this example cannot make it fail. This is the
  the same overclaim M11.6 caught, found the same way.

- [x] **M13.5** -- The replay-and-verify pass in [replay.rs](examples/epoch_log/replay.rs), which is
  what turns the sample from a demonstration into evidence. It is handed the watermark the log
  reported and holds it to exactly the contract's asymmetry: every record in an epoch at or below the
  watermark **must** be present, in sequence, with a matching payload and a validating checksum,
  while records above it may be present, absent, or torn and are counted rather than judged. Refusing
  to tolerate a torn tail would be its own bug -- the contract promises the tail is unreliable, so a
  reader that treated it as corruption would reject a healthy log.

  The example now appends `TAIL_RECORDS` into an epoch it **deliberately never commits**, because a
  replay pass that never sees an uncommitted tail has not been asked the interesting question.

  **The verifier is run three ways, and the third is the point.** Against the log as written (24
  durable verified, 3 tail tolerated); against a copy cut mid-record to simulate a crash (24 durable
  still verified, tail truncated, *no* violation reported); and -- the negative control -- against a
  copy with one byte corrupted **inside** the durable region, which must be reported. A verifier that
  cannot fail proves nothing, so the sample proves this one can: it reports
  `MissingDurableRecord { reason: ChecksumMismatch }` and the run states out loud that the checker
  which passed the first two cases was shown to be able to fail.

## Moved 2026-08-29 -- M14: crossing the ring boundary, and paying for the barrier

### M14 -- Worked example: crossing the ring boundary, and paying for the barrier

Second half of the example. M13 stays inside the ring; this milestone covers the two things that
forced the original consumer conversation -- operations the ring cannot express, and the cost of
[D-24](DESIGN-NOTES.md#d-24)'s full-barrier stall.

**Depends on M13.**

- [x] **M14.1** -- Order a non-ring operation against ring epochs: an `FSCTL`-class operation issued
  through [`windows-overlapped-io-sys`](../windows-overlapped-io-sys), sequenced at an epoch
  boundary, with its completion waited on in the *same* multiplexed wait as the ring's. This is the
  case `drain_preceding` cannot express at all ([D-24](DESIGN-NOTES.md#d-24) orders SQEs against
  SQEs), and the reason `completion_event` exists. Add the sibling crate as a dev-dependency.
  **Done:** `examples/epoch_log/reclaim.rs` reclaims a retired segment with `FSCTL_SET_ZERO_DATA`
  on a worker thread (that backend completes synchronously, which the event loop must not do), and
  `EventLoop` now waits on three handles. Two measured facts, both sabotage-checked: the reclaim
  running *alongside* appends does **not** make the third handle load-bearing (ring traffic wakes
  the loop; removing the handle costs nothing), whereas the idle-path reclaim after the ring
  quiesces does -- removing it turns a 78 ms run into a 30 s `WAIT_MS` block. The ordering itself
  is enforced by the log, not the ring, and asserting it before each request makes that checkable.

- [x] **M14.2** -- A control-plane and background path on `windows-threadpool-sys`: checkpointing or
  reclamation driven from the pool while the pinned log thread keeps the data path. Demonstrates the
  hybrid the design notes recommend -- Model B on the hot path, Model A for everything else -- in one
  program, which nothing in the crate currently shows. **Must also add an explicit `[[example]]`
  entry for `epoch_log` with `required-features = ["threadpool"]`** (found while doing M13.1): the
  sample uses no thread pool through M13, so it builds under `--no-default-features` today, and the
  moment this item introduces `EventDelivery` it stops -- which the `ioring-no-threadpool` CI job
  from M11.4 will catch as a build failure rather than a warning.
  **Done:** `examples/epoch_log/checkpoint.rs` runs the checkpoint on a *second* ring handed to
  `EventDelivery`; the pool thread that sees the covering flush complete is what authorises the
  reclaim, so the chain crosses log thread -> pool thread -> reclaim worker -> log thread with the
  log thread blocking for none of it. Two rings rather than one is forced by
  [D-21](DESIGN-NOTES.md#d-21), not chosen. The `[[example]]` entry was added and its necessity
  verified: removing it makes `cargo check --all-targets --no-default-features` fail with
  `unresolved import windows_ioring_sys::EventDelivery`, which is exactly what the M11.4 job runs.

- [x] **M14.3** -- Implement all three epoch-commit strategies from "Durability on the ring" behind
  one interface, selectable at run time: covering flush (ring stalls), host sequencing (a userspace
  round trip per epoch), and alternating rings (neither, at the cost of doubled registration). The
  point is that [D-24](DESIGN-NOTES.md#d-24) makes this a real fork with no free answer, and a
  reader needs to see all three to choose.
  **Done:** `examples/epoch_log/strategy.rs`. All three run the same workload and are checked two
  ways: each log replays clean, and all three must be **byte-identical** to each other. Both checks
  were sabotage-verified (a shifted offset trips replay; a dropped epoch trips byte-identity). The
  limit is stated in the code rather than papered over -- neither check can observe whether the
  ordering held on the *device*, which is only visible across a power cut, so the strategies are
  argued from [D-23](DESIGN-NOTES.md#d-23)/[D-24](DESIGN-NOTES.md#d-24) rather than from a clean run.

- [x] **M14.4** -- Measure the three strategies on the running machine and print the comparison:
  throughput, commit latency distribution, and ring idle time during the barrier. The example should
  *demonstrate* the trade-off rather than assert it, and the numbers are machine-specific enough that
  quoting ours would be misleading.
  **Done:** throughput, commit-latency quantiles, and append stall are measured per strategy and
  printed with an explicit "these describe THIS machine" note. The finding is that on this machine
  the three are **indistinguishable** -- the cross-strategy spread (1.08x-1.72x) is the same size as
  one strategy's run-to-run spread (1.42x) -- because every strategy pays one device flush per epoch
  at hundreds of microseconds while their actual differences land in the tens. The program computes
  and states that from its own data rather than hard-coding a ranking. Measurement also **found two
  harness bugs** that no other check caught: a single deferred-commit slot shared by two lanes (so
  half the commits were never awaited and `durable_through` was a claim), and pending commits keyed
  by `UserData` in one map across two rings whose sequences collide. Both are recorded in
  `strategy.rs` because both are easy to repeat.

- [x] **M14.5** -- Document the example: a module-level walkthrough, a pointer from `README.md` and
  from the "Durability on the ring" section of [DESIGN-NOTES.md](DESIGN-NOTES.md), and an explicit
  statement that it is a demonstration of a pattern rather than a supported API -- so that nobody
  vendors it and then expects this crate to maintain its policy choices.
  **Done:** `main.rs` opens with the not-API statement and its reasoning (D-8/D-26), a module table
  giving each file's job *and the contract behind it*, what one run does, and two things the sample
  cannot show. `README.md` points at the example from the Durability section and gains an "the
  examples are demonstrations, not API" subsection under Cargo features.
  [DESIGN-NOTES.md](DESIGN-NOTES.md) gains a subsection at the end of "Durability on the ring" that
  also promotes M14.4's two findings out of the sample, since both are about the design rather than
  about the demonstration.

## Moved 2026-08-30 -- M8 through M10: handle lifetime, cross-ring identity, and the contract audit

### M8 -- `FileRef::Raw(HANDLE)` lifetime safety (PR #20 review finding)

A caller can close or reuse the raw `HANDLE` passed to `Batch::read`/`write`/`flush`/`cancel`/
`register_files` before the kernel finishes with it: unlike a buffer (owned by a `Token` until claimed),
`FileRef::Raw` carries no lifetime and nothing in this crate borrow-checks a handle across the async gap
between push and completion.

- [x] **M8.1** -- Ownership model decided: `Batch::read`/`write`/`flush`/`cancel`/`register_files` become
  `unsafe fn` when addressing a raw `HANDLE` (their existing `SAFETY` comments already state the
  caller-keeps-it-alive obligation informally; this makes it a real, compiler-checked boundary), paired
  with a safe `SharedFile` wrapper (`Arc<OwnedHandle>`) for the common case: each push clones the `Arc` into
  the same `Token` that already tracks the operation's buffer (or, for `flush`/`cancel`, a standalone
  `Token<Arc<OwnedHandle>>`), so the underlying handle survives until every operation referencing it is
  claimed or leaked, regardless of what the caller does with its own `SharedFile` clone. Rejected:
  borrowing `FileRef::Raw` for the `Batch`'s own lifetime (does not solve the completion-outlives-the-push-
  call problem) and forcing every raw handle through an owning wrapper the way
  `windows-overlapped-io-sys`'s endpoints do (defeats `FileRef::Raw`'s zero-setup reason to exist).
  `FileRef::Registered` needs none of this and stays unaffected.

- [x] **M8.2** -- Added `SharedFile` (`pub struct SharedFile(Arc<OwnedHandle>)`) with a constructor from
  `OwnedHandle`, `Clone`, and a raw-handle accessor for building an `IORING_HANDLE_REF`.

- [x] **M8.3** -- Marked `Batch::read`/`write`/`flush`/`cancel`/`register_files`'s raw-`HANDLE`-taking
  forms `unsafe fn` with a `# Safety` section stating the real obligation (valid handle, correct access
  rights, remains valid until the pushed operation's completion is observed or the ring runs down).
  **Scope correction found during execution:** `read_registered`/`write_registered` take the identical
  `impl Into<FileRef>` shape for their own `file` argument and carry the same hazard, so they were marked
  `unsafe fn` too, with no separate checklist item -- the original enumeration omitted them by oversight,
  not by decision. Committed as `feat(ioring)!`.

- [x] **M8.4** -- Added safe overloads `read_shared`/`write_shared`/`flush_shared`/`cancel_shared` (plus
  `read_registered_shared`/`write_registered_shared`, for the same reason as M8.3's correction) taking
  `&SharedFile`, cloning the `Arc` into the same `Token` (a tuple with the buffer for `read`/`write`-shaped
  pushes, or `Token<SharedFile>` alone for `flush`/`cancel`, which have no buffer of their own) so a caller
  never needs `unsafe` for the common case. `register_files` gets no `_shared` counterpart: a registration's
  handles must stay valid for the ring's remaining life, a lifetime no single push's `Token` can express.

- [x] **M8.5** -- Integration test
  (`dropping_the_callers_own_sharedfile_clone_does_not_close_a_still_outstanding_handle`,
  `tests/submission_lifecycle.rs`): drop the caller's own `SharedFile` clone (its only external reference)
  while a push against it is still outstanding; the read still completes correctly against a live handle,
  proving the `Arc` clone inside the token -- not the caller's copy -- is what kept it open.

- [x] **M8.6** -- Renamed the API so the safe path gets the plain names: the raw/`unsafe` entry points
  became `read_raw`/`write_raw`/`flush_raw`/`cancel_raw`/`read_registered_raw`/`write_registered_raw`, and
  the safe `SharedFile`-taking overloads (previously `*_shared`) took over the plain names
  `read`/`write`/`flush`/`cancel`/`read_registered`/`write_registered` -- since the safe path is the one
  this crate wants to steer callers toward. `register_files` keeps its plain name unsafe, since it has no
  safe counterpart to make way for.

### M9 -- Cross-ring identity and registration-drop safety (PR #20 review findings)

`UserData`, `RegisteredFile`, and `RegisteredBuffers` indices are each meaningful only against the
specific ring that minted them, but nothing enforced that when more than one `IoRing` exists in the same
process.

- [x] **M9.1** -- Added `RingId` (`ring.rs`): a monotonic, process-lifetime-unique `AtomicU64` counter,
  never the ring's own `HANDLE` (which Windows can reuse for the next object after a ring closes). Every
  `IoRing` gets one at construction; every popped `Completion` now carries the id of the ring that
  produced it (D-17).

- [x] **M9.2** -- `Token::claim_if` now requires both the `UserData` identity and the `RingId` to match,
  closing the gap where two different rings' own zero-based `UserData` counters could coincide. Added
  `claim_if_rejects_a_matching_user_data_from_a_different_ring` (`src/token/tests.rs`).

- [x] **M9.3** -- `RegisteredFile`/`RegisteredFiles`/`PendingFileRegistration` and
  `RegisteredBuffers`/`PendingBufferRegistration` now carry the minting ring's `RingId`; `handle_ref`
  (fallible now) and a new `Batch::check_registration_ring` reject a `FileRef::Registered`/
  `RegisteredBuffers` argument from a different ring with `io::ErrorKind::InvalidInput`, checked before any
  `Build*` call runs. Added `a_registered_file_from_a_different_ring_is_rejected` and
  `a_registered_buffers_from_a_different_ring_is_rejected` (`tests/registration.rs`).

- [x] **M9.4** -- `PendingBufferRegistration` now leaks its buffers (via `ManuallyDrop` plus a
  deliberately empty `Drop`) if dropped without a matching `claim_if`, mirroring `Token`/
  `RegisteredBuffers` (D-18) -- previously it dropped `Vec<B>` normally, freeing memory the kernel might
  still reference from an already-queued `BuildIoRingRegisterBuffers` call. Added
  `dropping_an_unclaimed_pending_buffer_registration_leaks_rather_than_frees` (`tests/registration.rs`).

- [x] **M9.5** -- `Batch::do_submit` now marks itself attempted (`self.submitted = true`) *before*
  propagating `SubmitIoRing`'s `HRESULT`, not after: `submit_and_wait` takes `self` by value, so a failed
  call still runs `Drop` once its caller's `self` goes out of scope, and the old ordering left `Drop` free
  to silently retry an already-attempted submit -- which could succeed on the retry without the original
  caller's `Err` ever saying so.

- [x] **M9.6** -- Output-abstraction cleanup in the two examples the review flagged
  (`examples/model_a_delivery.rs`, `examples/l3_domains.rs`): routed through a single writer seam per the
  repository's architectural pre-step, matching `src/bin/run_scenario.rs` (in `windows-file-watcher`) and
  `examples/ring_copy/main.rs`'s existing pattern. `PLANS.md`'s bare `COMPLETED-PLANS.md`/
  `COMPLETED-CHECKLIST.md` references were also made clickable relative links.


### M10 -- Finish the contract audit against the ten specification-gap categories

[DESIGN-NOTES.md](DESIGN-NOTES.md) -> "Specifying this contract" audits this crate against
[the ten categories](../../DESIGN-NOTES.md#specifying-a-delivery-contract). The first pass reached four of
them: D-17's `RingId` (categories 4/5, handled correctly), `Completion::synthetic`'s test-only gate
(category 10, handled correctly), D-14's registration-index continuity (category 4, recorded as an explicitly
unverified assumption), and the previously-unstated completion-ordering rule. Categories 1, 2, 3, 6, 8, and 9
were **not examined**, recorded as "not looked at" rather than "does not apply".

M10.1-M10.3 closed that out, so all ten are now examined. The audit also turned up two API gaps rather than
mere statement gaps; they are M10.4 and M10.5 below rather than notes in the design record, per "design notes
are not a work queue".

- [x] **M10.1** -- Audited category 3 (state-dependent legality unenumerated) against capability negotiation
  ([D-28](DESIGN-NOTES.md#d-28)): [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "Category 3" now tabulates which
  `Batch` methods each probed op gates, and states the four rules the prose left silent -- `supports` answers
  for the kernel's op table rather than this crate's push surface (`Op::Nop` gates no `Batch` method at all,
  which is exactly the op M6+.3's shutdown case reaches for); `supports_raw` accepts named codes too and
  differs from `supports` in caching rather than truth, while never widening what `Batch` can push; every
  legality check runs before `reserve_user_data`, so a rejected push strands no identity; and the registration
  one-shot is enforced against the registered count, making the real rule "at most one registration that
  assigned an index" and making a *failed* registration unretryable on that ring. Corrected the `supports`,
  `supports_raw`, `Op`, `register_files`, and `register_buffers` rustdoc that stated these wrongly or not at
  all, and added an integration test binding the zero-length-registration rule.

- [x] **M10.2** -- Audited the remaining categories (1, 2, 6, 8, 9) against the ring/token/registration
  surface; [DESIGN-NOTES.md](DESIGN-NOTES.md) carries one section each. Category 1: file addressing and
  buffer addressing are genuinely orthogonal (all four combinations reachable), but file addressing and
  *safety* co-vary and in the wrong direction -- `FileRef::Registered` is reachable only through an
  `unsafe fn` whose safety obligation is vacuous for that very input
  ([D-29](DESIGN-NOTES.md#d-29), fixed in M10.4). Category 2: **every successfully queued SQE produces
  exactly one completion**, unconditionally -- the rule `run_down`'s termination silently depended on --
  plus the two "may" readings that were too weak (`submit*` returns entries submitted, not completed; a
  cancel is a request that yields a second completion rather than replacing its target's). Category 6: a
  completion matching no live `Token` is normal, via four distinct routes. Category 8: this crate joins
  nothing, deliberately, and now says so. Category 9: `io::Error::kind()` is lossy exactly where the
  `HRESULT` is not, discriminating this crate's own rejections and never the kernel's
  ([D-30](DESIGN-NOTES.md#d-30), predicate queued as M10.5). Category 7 falls out of 2 and 6 jointly, so
  all ten categories are now examined.

- [x] **M10.3** -- Resolved D-14's unverified registration-index continuity assumption, by **dissolution
  rather than measurement** ([D-31](DESIGN-NOTES.md#d-31)). D-14 justified the eager advance on the grounds
  that erring early "can only ever waste indices, never collide two registrations onto the same index" -- and
  that collision needs a *second* registration, which the PR #20 review response forbade the day after D-14
  was written. So `base_index` is always zero, no later base index is ever derived from the count, and the
  kernel's claim timing has no observable consequence; measuring it would settle a fact with nothing
  downstream of it. Took the checklist's second branch for the residue that *is* observable: the public
  counts now state on themselves that they report a **reserved** count, not a confirmed one -- already
  advanced before any completion is popped, and still advanced after a registration whose completion failed
  (which is why that registration cannot be retried, [D-28](DESIGN-NOTES.md#d-28)). Added a test binding
  reserved-at-queue-time, gave D-14 the required adjacent status marker, and corrected the audit section's
  category-4 paragraph, which still described the assumption as live.
  **Merge note (2026-08-29):** `main` had meanwhile recorded that the measurement *does* exist -- the
  2026-08-27 session established that a second `BuildIoRingRegisterFileHandles` replaces the whole table and
  re-bases indices at zero, that a table holds at least 65536 handles, and that an index is resolved at
  submission -- and scheduled the follow-up workspace-side as [M19.1](../../CHECKLIST.md), with the
  relaxation of the one-registration-per-ring rule as M19.2. See
  [DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md](../../design-sessions/DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md).
  That does not reopen this item, but it does bound the dissolution: **the dissolution's premise is that a
  second registration is forbidden**, so if M19.2 relaxes that rule, D-14's collision concern becomes live
  again and [D-31](DESIGN-NOTES.md#d-31) must be revisited rather than assumed to still hold. Recorded here
  because nothing else would carry that dependency across the two checklists.

- [x] **M10.4** -- Gave `FileRef::Registered` safe entry points ([D-29](DESIGN-NOTES.md#d-29)), via a sealed
  `FileTarget` trait with an associated `Guard` type ([D-33](DESIGN-NOTES.md#d-33)) rather than a parallel
  family of `*_registered_file` methods. `read`, `write`, `flush`, `cancel`, `read_registered`, and
  `write_registered` are now generic over it, so a `RegisteredFile` pushes without `unsafe` and the
  file-registered x buffer-registered combination is expressible for the first time. `Guard` carries the
  one real difference between the two targets -- what the `Token` must hold until the completion is
  observed: an `Arc` clone for `SharedFile`, nothing needing to be kept alive for `RegisteredFile` (which
  hands its `Copy` index back for symmetry). **Non-breaking**: `read(&SharedFile, ..)` still resolves to
  `Token<(B, SharedFile)>` and every existing call site compiled untouched; generic params are ordered
  `<B, F>` so an existing `read::<Vec<u8>>` turbofish still resolves. Sealing is load-bearing rather than
  tidiness -- an outside impl could return `FileRef::Raw(arbitrary)` with a guard keeping nothing alive,
  reintroducing what D-16 closed. Three tests: a safe registered-file read, the fully-registered
  composition, and a cross-ring rejection confirming D-17's check still holds on the safe path.

- [x] **M10.5** -- Added named predicates for the ring conditions a consumer must branch on
  ([D-30](DESIGN-NOTES.md#d-30), resolved as [D-34](DESIGN-NOTES.md#d-34)), in three parts. A
  `#[non_exhaustive]` `RingCondition` enum covering **every** `IORING_E_*` this crate names -- not only the
  actionable ones, since narrowing it would be the "narrow the platform to serve the visible goal" failure
  PLATFORM INTEGRITY forbids. Predicates (`is_submission_queue_full`, `is_completion_queue_too_full`,
  `is_submit_in_progress`) for just the runtime-actionable conditions, because a predicate asserts a branch
  exists; the rest stay reachable via `condition()`. And a sealed `IoRingErrorExt` putting those answers on
  `io::Error` itself, which is what actually removes the hand-rolled
  `get_ref().downcast_ref::<IoRingError>()` from call sites -- the crate's own integration tests had two
  such helpers, and deleting them was the new API's first use. `IoRingError::name` is now **derived** from
  `condition()` rather than matching the `HRESULT` a second time (CONTRACT INTEGRITY: prefer a derived fact
  to a restated one) -- previously a new condition could be added to one `match` and silently missed by the
  other. Did **not** map `IORING_E_*` onto `io::ErrorKind`; D-30's refusal stands. Bound to a *real*
  kernel-reported queue-full in the existing backpressure integration test, not only to synthetic errors.

- [x] **M10.6** -- Fixed a live use-after-free in `Batch::register_buffers`
  ([D-32](DESIGN-NOTES.md#d-32)). `BuildIoRingRegisterBuffers` reads its `IORING_BUFFER_INFO` array when
  the registration op *runs*, not when the `Build*` call returns; the crate built that array in a local
  `Vec` and dropped it before `SubmitIoRing`, so the kernel read freed heap and the registration completed
  with `ERROR_NOACCESS`. Since `register_buffers` is a **safe** `pub fn`, safe code could make the kernel
  dereference a dangling pointer -- a soundness hole in shipped 0.1.2, not a test defect. A spike crossed
  array-lifetime against buffer alignment and disproved alignment in both directions; it also established
  that the array may be released once submit returns, and that `BuildIoRingRegisterFileHandles` genuinely
  *does* read synchronously -- the asymmetry the rustdoc had wrongly generalized across, and the reason the
  file-registration tests always passed. The array is now owned by the `IoRing` rather than the `Batch`,
  because a failed submit leaves the SQE queued as ring state ([D-5](DESIGN-NOTES.md#d-5)) and a later
  unrelated submit can be what runs it. Added a regression test that churns the heap between the push and
  the submit, verified by sabotage; corrected both false "read synchronously" claims; moved the entry from
  [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md) to
  [RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md).

## Moved 2026-08-30 -- M15 through M18: the testing-strategy response to eight defects

### M15 -- Deterministic memory instrumentation (release-gating)

Eight defects came out of the 0.1.x line and the M11-M14 branch:
[#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47),
[#48](https://github.com/MikeGrier/windows-threadpool-sys/issues/48), `get_mut` returning `&mut B`
([D-35](DESIGN-NOTES.md#d-35)), `Appender::claim` leaking an arena slot, the checkpoint path authorising a
reclaim after a failed write, a shared deferred-commit slot in the strategy harness, `get` racing the kernel
([D-36](DESIGN-NOTES.md#d-36)), and a `debug_assert` that could never fire. Sorting them by *what would have
caught them* is what M15-M18 are built from, because they need different tools and one population needs a tool
that does not exist:

- **A -- preconditions never varied.** Every `tests/event_delivery.rs` case handed over a *fresh* ring, so
  "completion queue non-empty at handover" was never a test input. That is
  [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47). M17.
- **B -- failure paths never taken.** No test ever ran `completion.result()` returning `Err`, which is the
  checkpoint defect, and is why [#48](https://github.com/MikeGrier/windows-threadpool-sys/issues/48) surfaced
  as a *lucky* `ERROR_NOACCESS` rather than as corruption. M15 and M16.
- **C -- permissions, not behaviour.** `&mut Vec<u8>` *permits* `reserve`/`resize`/reassign; no execution ever
  performs it. That is [D-35](DESIGN-NOTES.md#d-35) and [D-36](DESIGN-NOTES.md#d-36), the two most severe
  findings, and **no runtime technique reaches this population at all.** M18.

**Deliberately rejected: a mock `IoRing`.** Both shipped defects were the kernel behaving differently from
this crate's assumptions -- the completion event is edge-triggered ([D-19](DESIGN-NOTES.md#d-19)), and
`BuildIoRingRegisterBuffers` reads its array at submit rather than at build ([D-32](DESIGN-NOTES.md#d-32)). A
mock encodes the assumption, so one written before those discoveries would have passed both bugs green -- it
would not merely have failed to find them, it would have manufactured evidence they were absent. A model
belongs here as an **oracle over observed sequences** (M16), never as a substitute for the kernel.

**Deliberately rejected: Application Verifier / PageHeap**, after measuring it rather than assuming either way.
See [D-37](DESIGN-NOTES.md#d-37); the short version is that it works but is keyed by *image file name*, and
cargo rehashes test binaries on every meaningful rebuild.

**M15 and M16 gate the 0.2.0 release; M17 and M18 do not.** 0.2.0 carries two security fixes and should not
wait on the state-space work.

- [x] **M15.1** -- Add a guard-page global allocator for test builds: `VirtualAlloc` each allocation with a
  trailing `PAGE_NOACCESS` guard page, right-aligned so its end abuts the guard, and on free flip the block to
  `PAGE_NOACCESS` and **never reuse the address**. That combination makes both an overrun and a
  use-after-free a deterministic `0xC0000005` instead of a silent stale read.
  Use `MEM_DECOMMIT` rather than plain `VirtualProtect` on free: never reusing an address is the point, but
  never releasing the *commit* would grow a long suite without bound.
  A working probe is at `.scratch/guardalloc/` -- measured clean/UAF/overrun as `0`/`0xC0000005`/`0xC0000005`,
  against a system-allocator baseline that silently read a freed byte and exited `0`.
  **Sabotage-verify against the real defect:** revert [D-32](DESIGN-NOTES.md#d-32)'s fix locally and confirm
  the allocator turns it from a lucky `ERROR_NOACCESS` into a deterministic failure. An instrument never seen
  to fire is indistinguishable from one that cannot.
  **Done:** new `windows-guard-alloc` crate (`publish = false`), installed by `tests/registration.rs`. The
  calibration was run and is the point of the item: reverting [D-32](DESIGN-NOTES.md#d-32) makes
  `a_buffer_registration_survives_heap_churn_between_the_push_and_the_submit` -- the regression test that fix
  added -- die with `STATUS_ACCESS_VIOLATION` instead of passing. Five subprocess tests in `tests/faults.rs`
  prove the allocator fires at all (an access violation cannot be caught in-process), and a
  `the_guard_allocator_is_installed_for_this_test_binary` assertion catches the silent failure mode where the
  `#[global_allocator]` attribute is missing and everything passes uninstrumented.

- [x] **M15.2** -- Fill every allocation and every freed block with a **tracked** poison pattern, derived from
  a per-run seed plus a per-allocation ordinal so the bytes identify *which* allocation they came from rather
  than merely being "not real data". A fixed constant like `0xDD` is worth much less: it collides with real
  payloads, and it cannot answer "where did this come from".
  The tracking is also what keeps this compatible with this component's reproducibility rule: **log the seed at
  test start and accept it from the environment**, so a failure is replayable exactly rather than being a
  one-off. Record that in [DESIGN-NOTES.md](DESIGN-NOTES.md) as the terms under which non-fixed test data is
  permitted here.
  Keep the pattern cheap to write (a repeating 8-byte word carrying the ordinal, not a per-byte hash) since it
  runs on every allocation.
  **Done:** `windows-guard-alloc`'s `poison` module. The pattern is `splitmix64(seed ^ ordinal)` repeated
  across the allocation, and the mixing function is a **bijection with a computable inverse**, so
  `GuardAlloc::poison_check` recovers *which* allocation a region belongs to from its own leading bytes --
  no snapshot needed, which is what M15.3 will build on. Seeding terms recorded as
  [D-39](DESIGN-NOTES.md#d-39) and verified end to end: two unpinned runs differ, two pinned runs reproduce
  byte-for-byte, and both decimal and `0x` hex parse.
  **Two findings while implementing.** (1) *Poisoning freed blocks is dead code here* -- `dealloc` decommits,
  so those bytes are unreadable and poison written there could never be observed by anything. It would cost a
  memset per free for a guarantee strictly weaker than the one already in force, so it is deliberately not
  done, and `dealloc` says why. The item text above is left as originally written rather than quietly edited,
  since the planning error is the more useful record. (2) The first draft **hardcoded two multiplicative
  inverses and both were wrong**; `the_multiplicative_inverses_are_actually_inverses` caught it. They are now
  *derived* by a `const fn` Newton iteration, which makes that class of error unrepresentable rather than
  merely tested for.

- [x] **M15.3** -- Verify the poison at the points where the *kernel* is the suspect, which is the gap guard
  pages structurally cannot cover: a guard page only catches access to memory that should not be touched at
  all, and says nothing about the kernel writing into a live, valid allocation. Both checks below are
  invariants this crate asserts in prose today and verifies nowhere:
  - **After a registered *write* completes** (`KernelAccess::ReadsBuffer`): the slot must be **byte-identical**
    to what was submitted. The kernel is only permitted to read it.
  - **After a registered *read* completes** (`KernelAccess::WritesBuffer`): every byte *outside*
    `[span.offset, span.offset + information)` must still be poison. This binds the kernel to the
    `RegisteredSpan` we declared, and would equally catch our own `checked_span` or offset arithmetic being
    wrong.
  Sabotage-verify each by deliberately submitting a span narrower than the write, and by mutating a slot
  mid-flight.
  **Done:** `tests/kernel_span.rs`, five tests. Both named checks, plus two the item did not anticipate: a
  **short read** must leave the unfilled remainder of its span poison (the span is a permission, not a
  promise, so `information` is the only thing that says where real data stops), and a read into one slot must
  leave its **neighbours** untouched -- which is what per-slot ordinals buy over a single constant, since a
  write landing in the wrong slot is visible as such rather than merely as "something changed".
  All four assertions were sabotage-verified individually and each named the right failure: a byte before the
  span, a byte past its end, a modified write source, and a disturbed neighbour. Every message carries the
  seed, so a failure is replayable per [D-39](DESIGN-NOTES.md#d-39).
  **Note the checks cut both ways**, which is worth more than the kernel-conformance reading: the offset the
  kernel receives is computed by *this* crate in `checked_span`, so an off-by-one there fails these tests
  exactly as a kernel bug would -- and is otherwise as invisible as one.

- [x] **M15.4** -- Verify poison at quiescence too: at teardown every registered buffer's never-written
  regions still hold their expected pattern, and no buffer that was only ever a write source has changed.
  This is the whole-arena form of M15.3 and is what catches a stray write attributed to no particular
  operation.
  **Done:** `windows-guard-alloc`'s `witness` module plus
  `a_mixed_workload_leaves_every_unaccounted_byte_poisoned`. A `Witness` starts owning a region with nothing
  permitted; each completion permits exactly the bytes it was entitled to change -- the **transferred** count,
  never the requested one -- and `verify` walks the *gaps* between merged permissions at teardown. Slot 3 in
  the test is a write source only, so nothing is ever permitted for it and it must be byte-identical.
  Both directions were sabotage-verified: a byte in the gap between slot 0's two disjoint reads, and a byte in
  the write-source slot, each reported with the slot, the offset, expected-versus-found, the seed, and how
  many bytes were legitimately written (`0 byte(s)` for the write source). The 12 `witness` unit tests weight
  the **false-positive** direction deliberately -- abutting ranges, overlapping ranges, out-of-order
  permissions -- because a witness that accuses legitimate writes trains a reader to ignore it, which is the
  same failure as missing one.

### M16 -- Make the contract executable, and take the failure paths (release-gating)

Population B, and the invariants this crate already *states* but nothing checks.

**Depends on M15** only for convenience -- the oracle is independent of the allocator, but the two are most
useful together.

- [x] **M16.1** -- Add `src/contract.rs` with a public `RingContract`: the executable form of
  [the category-2 rule](DESIGN-NOTES.md#one-sqe-one-completion) -- "every SQE that successfully queues
  produces exactly one completion" -- every token claimed-or-deliberately-leaked, and every per-buffer
  outstanding count back to zero at quiescence.
  **Correction (found while implementing):** this item originally cited D-29/D-30 for that rule. Both are
  about something else -- D-29 is `FileRef::Registered`'s safety and D-30 is `io::Error::kind`'s asymmetry --
  and the rule actually lives in the M10.2 audit's category-2 prose, which had no anchor to cite. It has one
  now. The wrong citation had already reached [PLANS.md](PLANS.md) and
  [DESIGN-NOTES.md](DESIGN-NOTES.md) itself; all three are corrected. Fed by the caller (`observe_push` / `observe_completion` / `check_quiescent`) rather than
  wired into `Batch`, so a consumer can validate its own harness against the same definition.
  It lives in **this** crate, not in the test harness: the layer that owns the invariant owns the oracle, and a
  harness-side copy is a second implementation of the rule rather than a check of it. Follow
  [`ContractChecker`](../windows-file-watcher/src/contract.rs)'s shape.
  State plainly what it does **not** check -- it cannot observe device ordering or anything absent from the
  completion stream -- since over-constraining is the same defect as under-specifying.
  **Done:** `src/contract.rs`, 16 unit tests. Five violations: unexpected completion, duplicate completion,
  outstanding at teardown, leaked token, buffer still in use. A completion provisionally marks its operation
  **leaked**, corrected by `observe_claim` -- so forgetting to claim is the default failure rather than the
  silent success, which is the shape of the `Appender::claim` defect. A leak is excused only when *stated*
  via `observe_deliberate_leak`, because the difference between a stated and an unstated leak is the whole
  point. Violations sort before they are reported, since `HashMap` order is unspecified and an oracle whose
  output reorders between runs cannot be diffed. Half the tests are the **does-not-fire** direction, listed
  in the module doc: completion order (the ring promises none between independent operations, and
  [D-24](DESIGN-NOTES.md#d-24)'s barrier constrains execution rather than pop order), operation success,
  anything about the device.

- [x] **M16.2** -- Bind the existing integration tests to `RingContract` and assert quiescence at teardown.
  This is the item that pays for M16.1: the `Appender::claim` slot leak and the strategy harness's shared
  deferred slot were both conservation failures, found by review and by measurement respectively, and both
  would have fallen out of a quiescence assertion automatically.
  Sabotage-verify by reintroducing one of them and confirming the oracle reports it.
  **Done:** bound `tests/submission_lifecycle.rs` (two tests) and `tests/kernel_span.rs`'s mixed workload,
  which also reports per-buffer counts. **The example's `Appender` now owns a `RingContract`**, because the
  claim that the `Appender::claim` leak "would have fallen out automatically" had to be *proved* rather than
  asserted -- and nothing outside the example could prove it. Calibrated against the real defect: taking the
  pre-fix early-return path once makes the run die with `the token for user_data 0x1 was dropped unclaimed,
  leaking whatever it held for the life of the process`.
  **Binding real tests exposed a gap in M16.1**: `flush_raw` and `cancel_raw` return a bare `user_data` with
  no token, so the oracle demanded a claim that cannot be made -- a false violation the caller has no way to
  satisfy, which is the worst kind. Added `observe_tokenless_push` and a `PushedTokenless` state, with four
  more tests. The backpressure test also exercises the rule's other half: the *rejected* push is deliberately
  never observed, since a synchronously-failing `Build*` releases its reservation and produces no completion.
  Noted for M16.3: reverting the fix's *ordering* alone does not reproduce the leak, because the early return
  only triggers on a **failed** write -- which is precisely the population-B path that has no coverage yet.

- [x] **M16.3** -- Add a fault-injection seam at the completion boundary, so a test can make `result()` return
  a chosen error for a chosen operation. `Completion::synthetic` already exists under `#[cfg(test)]` and is
  half of this. Keep it test-gated for the same reason `synthetic` is: `Token::claim_if`'s safety argument
  depends on every `Completion` tracing back to a real popped `IORING_CQE`.
  **Done:** `Completion::with_injected_failure` plus an `InjectedFailure` enum (`Ring` / `Win32` / `Hresult`),
  behind a new default-off `fault-injection` feature.
  **The design turns on a distinction the item's framing missed.** `synthetic` *fabricates* a completion, and
  the reason it must stay `#[cfg(test)] pub(crate)` is not merely convention: a fabricated completion can name
  an operation still **in flight**, so claiming a token against one hands a buffer back while the kernel is
  writing through it -- the exact use-after-free this crate exists to prevent. Building the seam that way to
  test safety would be self-defeating. So this seam **transforms a completion the ring genuinely popped**
  instead: same `UserData`, same ring identity, same finished operation, and only `result()` changes.
  `claim_if`'s argument is therefore untouched, which is what makes the feature safe to expose at all.
  **Found while writing the tests:** `Hresult(0)` would have injected *success*, silently falsifying the
  "failure only, never success" guarantee and letting a test conceal the very defect it was written to find.
  Now enforced by a panic rather than asserted in prose, with a `should_panic` test on it.
  Also deleted a test that could not verify its own name -- `information` is private and unreachable once
  `result()` is `Err`, so "reports no bytes transferred" had no assertion to write. The zeroing stays (it
  would be wrong to model a state the kernel never produces) but the vacuous coverage does not.
  `tests/fault_injection.rs` proves the seam is reachable *and* gated from outside the crate; CI's existing
  `--workspace --all-features` job runs it, verified rather than assumed.

- [x] **M16.4** -- Use M16.3 to cover the failure paths that have no coverage at all: a failed read, write and
  flush completion claimed normally, a failed registration, and `EventDelivery` delivering a failed completion.
  Assert the *documented* degradation each time rather than merely that nothing panics -- the checkpoint defect
  was precisely a failure that was noticed, recorded, and then not acted on.
  **Done:** `tests/failure_paths.rs`, six tests, all five named cases plus the loop-closer M16.2 left open --
  `claiming_before_checking_the_result_is_what_stops_a_failure_from_leaking` runs *both* orderings against
  `RingContract` on the same injected failure, so the `Appender::claim` defect is a reported violation rather
  than an argument. `EventDelivery` uses a **genuine** `ERROR_NOT_FOUND` from cancelling a non-outstanding
  target rather than an injected failure, per M16.3's own guidance.
  Two sabotage-verified: making a failed registration leak instead of drop, and making `EventDelivery` swallow
  failed completions. Both fired with the right message.
  **Corrected M16.3 while doing this.** That item's soundness argument covered `Token::claim_if` only, and
  `PendingBufferRegistration::claim_if` is materially different: it treats a failed completion as proof the
  kernel did *not* retain the addresses, so it **drops the buffers**. Injecting a failure there frees memory
  the kernel genuinely holds registered. Inert only while nothing uses it -- the test takes the precaution
  explicitly and `with_injected_failure` now documents the limitation, which it did not before.
  **Found a race in the test, not the crate:** the `EventDelivery` callback published the error code *after*
  the counter the waiting thread spins on, so the waiter read a stale zero. Fixed by publishing the data
  before the flag -- the same ordering rule the epoch log's reclaim worker already follows.

### M17 -- Feed the detectors M15 and M16 built

Population A. [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47) is one point in a space
nobody was sampling: `{fresh, dirty}` handover state crossed with operation kind, buffer kind, claim/drop and
drain timing. Enumerating that by hand is how it was missed the first time.

**What M15 and M16 changed about this milestone's purpose.** Neither found a single new defect in shipping
code: every finding was either historical ([D-32](DESIGN-NOTES.md#d-32), `Appender::claim`) or a defect in the
new instrumentation itself. That is not evidence the crate is clean. `RingContract`, the guard pages, the
poison, `Witness` and the fault seam are all **passive** -- they fire only on paths the existing hand-written
tests already walk, and #47 lived on a path no test walked. M17 is therefore not a fourth technique standing
beside M15 and M16. It is the **input generator for detectors that are already built and currently under-fed**,
which is also why its cost is lower than when it was first planned: the oracle, the memory checking and the
failure paths all exist now.

**Depends on M16.1** (the oracle is the property these tests assert).

**CONVENTION GATE -- CLEARED.** Randomized property testing is **permitted**, decided by the engineer and
recorded as [D-41](DESIGN-NOTES.md#d-41), so M17.3 is unblocked. Two conditions bind it: seed whatever can be
seeded (so a run replays from one number, and varying that number is how permutations are generated), and
where non-determinism is inherent and outside our control -- multiprocessor scheduling, kernel completion
order -- randomness is fair game. The corollary that keeps the second from becoming a loophole: inherent
non-determinism excuses *irreproducibility*, never *unverifiability*, so a test that cannot be replayed must
instead prove it reached the state it claims to exercise.

- [x] **M17.1** -- Close the two open cells of the handover precondition. A coverage inventory taken after M16
  found this axis is mostly covered already, so this is no longer a matrix to build: *fresh* and
  *queued-non-empty* are covered by [tests/completion_event.rs](tests/completion_event.rs),
  *drained-then-resubmitted* by its `the_edge_re_arms_after_every_drain_to_empty`, and #47's own repro by
  [tests/event_delivery.rs](tests/event_delivery.rs). What remains is (a) attaching while operations are
  **in flight but no completion has landed**, for both `IoRing::completion_event` and `EventDelivery::new`,
  and (b) drain-then-resubmit for `EventDelivery`. Gated on nothing; done first so #47's own axis is banked
  before any convention decision.
  **Done:** [tests/handover.rs](tests/handover.rs), four tests, each bound to `RingContract` so a lost,
  duplicated or unclaimed completion is a contract violation rather than an inferred count.
  **The planned approach for cell (a) was wrong, and its sabotage is what proved it.** The item called for
  racing the attach against in-flight operations. That state does not exist for *buffered* reads: they
  complete **synchronously inside** `submit_and_wait`, measured at 80 of 80 attempts across five shapes up to
  512 reads of 64 KiB, always leaving a full queue at attach time and never a partial split. The first draft
  swept a delay across the supposed window and passed; sabotaging [D-20](DESIGN-NOTES.md#d-20)'s setup signal
  failed it on *attempt 0*, revealing the sweep sat entirely on the already-queued side and the test was a
  slower restatement of `attaching_to_a_ring_whose_queue_is_already_non_empty_still_signals`. Recorded as
  [D-40](DESIGN-NOTES.md#d-40). Cell (a) is covered two ways instead. **Deterministically:** one attach
  serving a backlog queued *before* it **and** a wave submitted *after* it into the still-non-empty queue,
  which is where a seam between the setup signal and the edge would strand work exactly as #47 did.
  **And genuinely in flight:** unbuffered (`FILE_FLAG_NO_BUFFERING`) reads *are* asynchronous, so the state
  is reachable after all -- reusing the aligned-buffer shape [tests/flush_barrier.rs](tests/flush_barrier.rs)
  already had, extended to `IoBufMut`. That test **checks its own precondition**, counting what was already
  queued at the instant of attach and failing if no attempt caught a read in flight, so it cannot decay into
  the already-queued case on faster hardware; dropping only the `NO_BUFFERING` flag makes it fail with
  exactly that message.
  **Calibrated, and the two mechanisms are separable:** suppressing the setup signal fails only the two
  mixed-queue tests; suppressing `EventDelivery`'s `activation.rearm` fails only the re-arm test. Disjoint
  failure sets, so neither test is passing on the other's behalf.
  **Cell (b) was genuinely open, shown by mutation rather than asserted:** with `activation.rearm` removed,
  **all 107 pre-existing tests still pass** -- the entire suite was blind to `EventDelivery` silently
  ceasing delivery after its first drain -- and only the new test catches it. The three targets that did not
  get to run reference no `EventDelivery`, so they could not have caught it either.
  Whether other test files should also bind to `RingContract` is left to **M18.3**'s `cargo-mutants` triage
  to answer with evidence, rather than retrofitted blind now.

- [x] **M17.2** -- Decide and record whether randomized property testing is permitted here, as a
  [DESIGN-NOTES.md](DESIGN-NOTES.md) decision. [D-39](DESIGN-NOTES.md#d-39) already fixed the terms under which
  non-fixed test data is allowed in this component -- **seeded, announced, pinnable** -- and M15.2 proved them
  end to end, so this is now the narrow question "does D-39 extend from poison to `proptest`?" rather than an
  open-ended policy call. Inherit D-39's three terms rather than re-deriving them, and add the two it does not
  cover: a committed regression corpus so any discovered failure becomes a permanent fixed case, and placement
  as **integration** tests rather than unit tests (they cross the OS boundary and will exceed the one-second
  unit budget).
  **Decided: permitted**, recorded as [D-41](DESIGN-NOTES.md#d-41). D-39's three terms carry over to any
  generator, and the two operational terms above are adopted. The engineer added a **second condition D-39 did
  not anticipate**: where non-determinism is inherent and outside our control -- multiprocessor scheduling,
  kernel completion order, thread-pool dispatch -- randomness is fair game, because a test depending on those
  cannot be seeded and pretending otherwise would make it deterministic in its inputs while still
  nondeterministic in what it observes. D-41 records the corollary that stops that becoming a loophole:
  inherent non-determinism excuses *irreproducibility*, never *unverifiability*, so an unreplayable test must
  prove it reached the state it claims to exercise. M17.1 is cited as the worked example of both halves -- its
  racing draft decayed into testing the easier state, and its replacement guards against exactly that.
  No dev-dependency is added here. D-41 permits randomized property testing without mandating a particular
  crate, and M17.3 went on to satisfy its terms with a seeded `SplitMix64` rather than `proptest` -- see that
  item for why.

- [x] **M17.3** -- Model the operation space as data: operation kind, buffer kind (owned / registered), file
  target kind (raw / shared / registered), claim-or-drop, drain-now-or-later, and handover state. A generator
  over *sequences* of these, each sequence checked against `RingContract`. The harness **must run under
  `windows-guard-alloc` with poison enabled**: if it does not, generated sequences are checked for conservation
  only, which discards every M15 detector at exactly the moment there is finally enough input to feed them.
  Address space is not a constraint on doing so -- measured at 8 KiB per allocation, about 8.6e9 allocations
  before 64 TiB, against a generative run needing a few million.
  Bound by [D-41](DESIGN-NOTES.md#d-41): the generator is **seeded, announced and pinnable**, so one number
  replays a whole run and varying it is how permutations are produced. Where a sequence's outcome also turns
  on scheduling or kernel completion order, that residue is legitimately random -- but D-41's corollary still
  applies, so any such sequence must **verify it reached the state it claims**, not merely fail to crash. Note
  the guard allocator's seed and the generator's are two separate knobs and must not be conflated in the
  announcement, or a replay will reproduce one and not the other.
  **Done:** [tests/generated_sequences.rs](tests/generated_sequences.rs). 128 sequences per run, ~650
  operations, all 15 reachable shapes, under the guard allocator and checked against `RingContract`.
  **No `proptest`.** D-41 permits randomized property testing without naming a crate. Its terms are that one
  number replays a whole run and is pinnable from the environment; `proptest`'s model is a persisted
  regression file plus per-case seeds, a different shape, and its headline feature -- shrinking -- earns its
  keep on long sequences. These cap at 10 steps and print every one, so a failure already arrives readable. A
  seeded `SplitMix64` over one `u64` serves the stated terms exactly. If the step cap is ever raised
  substantially, shrinking stops being redundant and the choice should be revisited; that condition is
  recorded in the file's module doc rather than left implicit.
  **Seeding verified end to end:** two runs pinned to the same pair of seeds produced byte-identical coverage;
  a different seed produced different sequences; both decimal and `0x` hex parse. The two seeds are
  independent knobs and both are announced with the replay command.
  **The generator is self-verifying, per D-41's corollary.** A green run proves nothing unless it shows which
  states it reached, so the test asserts every shape appeared, plus at least one deliberate drop, one
  mid-sequence drain, and one attach against a ring with work outstanding (#47's own axis). Sequence count is
  set *by* that assertion rather than by taste: at 48 the rarest shape (flush against a registered file, 3.3%
  of operations) would be missed about 1 run in 4000 and flake CI; 128 puts that at ~3e-10.
  **The first run found a defect in the generated program, not the ring.** It emitted a registered-buffer
  operation whose token was deliberately dropped, and `RegisteredBuffers`'s drop guard fired.
  `read_registered_raw`'s rustdoc already states that token "must be claimed", because claiming is the only
  thing that releases the use -- `Token`'s drop is deliberately empty (D-4), so dropping instead pins that
  buffer index for ever. Claim-or-drop is therefore a free axis only for owned buffers, which the generator
  now encodes alongside the existing rule that two live operations never share one registered buffer index.
  **Calibrated by sabotage, which also exposed a defect in the harness:** removing `RegisteredUse::drop`'s
  `outstanding` decrement -- D-32's own class of bug -- is caught and reported as
  `BufferStillInUse { index: 0, outstanding: 1 }` with the seed and the step that did it. The first attempt
  reported it as a bare `debug_assert` from inside the crate instead, because the registration's drop guard
  fires *while the error is being returned* and masked the diagnostic. The failure path now forfeits the
  registration deliberately -- the same leak-not-free choice the crate makes in release -- so the report
  survives. An instrument whose failure output names neither sequence, step, nor seed is barely an instrument.
  Note this sabotage is a *smoke test* that the harness is not inert, not the calibration M17.4 requires:
  it shows the oracle reports a fault injected into buffer accounting, not that the generator can emit the
  shapes that trigger the defects which actually shipped.

- [x] **M17.4** -- Calibrate the generator before its green result is allowed to count. Show that it
  rediscovers a defect known to be real -- #47's handover shape, or D-32's registration timing -- when the fix
  is reverted. Every M15 and M16 item carried a sabotage step and this milestone had none: a generator that
  cannot emit the shape that triggers #47 reports green for ever, and that green would feed straight into
  release confidence. Until this item passes, M17.3's result is uninformative by construction. Note M18.3's
  `cargo-mutants` run is a general form of the same calibration; this targeted version comes first because it
  is cheaper and aims at two known-real defects rather than synthetic mutants.
  **The calibration failed the generator, which is the entire reason this item exists.** With
  [D-20](DESIGN-NOTES.md#d-20)'s setup signal removed -- #47 exactly as it shipped -- the M17.3 generator
  reported **green**. It attached the completion event but drained with `try_pop`, and unconditional polling
  recovers every completion whether or not the ring ever signalled, so a lost wakeup was invisible to it. A
  generator can sample the right *states* and still be blind to the defect that lives in them.
  **Fix:** once an event is attached, a sequence now **waits for the wakeup it is owed** before draining, and
  a wait that times out with work still outstanding is reported. Waiting precedes draining rather than
  following it, because a backlog queued *before* the attach is reachable only through the setup signal --
  drain-first would consume it by polling and hide the signal's absence. Recorded as
  [D-42](DESIGN-NOTES.md#d-42).
  **Re-calibrated, and the detection is reliable rather than lucky:** with the sabotage restored the generator
  caught it in **10 of 10 runs on fresh seeds**, always within the first six sequences, and the reported trace
  is #47's own shape -- an operation queued, `attach completion event (queue had 1 outstanding)`, then a lost
  wakeup. With the fix in place, 10 of 10 clean runs at ~80 ms.
  **Second defect, D-32:** reintroducing the 0.1.2 use-after-free (build the SQE from an array dropped before
  submit) is caught as a hard `STATUS_ACCESS_VIOLATION`. Worth recording precisely, because it is *not* what
  0.1.2 did: there the same defect surfaced as a survivable `ERROR_NOACCESS` because the freed pages happened
  to still be mapped. The only difference is the guard allocator decommitting them, which is M15.1's stated
  purpose demonstrated against the real historical defect rather than a synthetic one. Note this one is caught
  at the registration *setup* boundary, which every registering test crosses, so it calibrates the allocator
  rather than the generated space -- #47 is the calibration that is specifically about the generator.
  **Process note:** an intermediate "failure" run was traced to a stale test binary left by the previous
  sabotage build, not to flakiness. Timestamps were checked against the rebuild before any conclusion was
  drawn from a direct binary run; a calibration that mistakes a stale artefact for a result is worse than none.

- [x] **M17.5** -- Make discovered failures permanent: every sequence the generator finds becomes a named,
  seed-free regression test committed alongside the corpus. A property test that finds a bug and then forgets
  it has bought a single debugging session rather than a guarantee.
  **The trap in this item is that the generator has found no defect in shipping code**, which makes "nothing
  to make permanent" look like a reason to defer. It is not: the first real failure is exactly the moment
  nobody wants to be inventing a corpus format, so the mechanism is built and proven now.
  **Corpus:** named, seed-free entries replayed verbatim through the generator's own executor, each carrying
  *why* it was recorded. Seeded with `regression_issue_47_backlog_at_handover` -- #47's shape exactly as M17.4
  reported it, kept in the shape it was found rather than tidied into a minimal one, because a corpus that
  rewrites what it was given cannot be trusted to have preserved the failing case. This is a real
  strengthening rather than bookkeeping: the generator reaches that shape within a handful of sequences on
  *most* seeds, and the corpus reaches it on *every* run.
  **Verified load-bearing:** with [D-20](DESIGN-NOTES.md#d-20) reverted the entry fails deterministically,
  reporting "reproduced a defect that was fixed", the recorded reason, and the step trace.
  **The calibration itself is now automated**, which M17.4's could not be. That procedure edits `src/`, so
  nothing in CI would notice if `wait_then_drain`'s wait were later "simplified" back into a poll -- the exact
  change that made the generator blind to #47 ([D-42](DESIGN-NOTES.md#d-42)).
  `the_lost_wakeup_detector_fires_when_no_wakeup_is_owed` constructs the lost-wakeup condition against a
  *correct* ring instead -- attach, consume the setup signal, then wait again without draining, so no
  empty-to-non-empty edge can occur -- and asserts the detector reports it. Sabotaging the wait makes it fail,
  so the guard is real. A short 250 ms budget is used because that wait is *expected* to expire; `WAIT_MS`
  would add five seconds per run to learn the same thing.
  **Found while adding tests:** `temp_file` keyed its fixture on the process id alone, so the second test in
  this binary would have raced the first for one path -- libtest runs them as concurrent threads. Now tagged
  per test.
### M18 -- The population no runtime technique reaches

Population C, and the one that produced the two most severe findings of the M14 review round. `get_mut`
returning `&mut B` ([D-35](DESIGN-NOTES.md#d-35)) and `get` returning an unchecked `&B`
([D-36](DESIGN-NOTES.md#d-36)) are both defects of **what safe code is permitted to do**, not of what any code
path does. No fuzzer, oracle, allocator, poison scheme or chaos harness reaches them, because nothing has to
execute for the hole to exist. Both were found by review, which makes review a *primary* technique here rather
than a backstop -- and that deserves a written procedure instead of depending on who happens to read the diff.

Independent of M17; may run in parallel.

- [x] **M18.1** -- Audit every public type that hands out a borrow or an owned value, against one mechanical
  question: **what can safe code do with this, and does the registration or the kernel still hold anything it
  could invalidate?** `&mut Vec<u8>` permits `reserve`, `resize` and whole-value assignment; `&mut [u8]`
  permits none of them. Record the finding per item even when the answer is "nothing", so the absence of a
  hole is evidence rather than silence.
  **Done:** [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [Borrow-surface audit](DESIGN-NOTES.md#borrow-surface-audit-m181),
  nineteen items, each recorded including the sixteen where the answer is "no hole".
  **One finding, and it is the same shape as the two that prompted the audit.** `EventDelivery::ring` hands
  out `&Mutex<IoRing>`, and *any* `&mut IoRing` permits whole-value assignment, so safe code can replace the
  ring and silently stop delivery. Measured rather than argued: the replacement compiles, and a probe recorded
  one completion delivered before the swap and **none** after, despite four further operations completing on
  the replacement. The pool's wait holds a duplicate of the *original* ring's event, and nothing is ever
  attached to the new one. Recorded as [D-43](DESIGN-NOTES.md#d-43); the fix is **M18.6** below.
  **Worth carrying into M18.2:** the question that finds these is not "is this correct?" but "what else does
  this type allow?" Note also that the two obvious fixes do not work -- a `Deref`/`DerefMut` newtype still
  permits `*guard = ...`, and so does a `with_ring(|ring: &mut IoRing| ...)` closure. Anything that lets a
  `&mut IoRing` escape keeps the hole.
- [x] **M18.2** -- Turn M18.1 into a recurring obligation rather than a one-time pass, as a
  `DESIGN-INSTRUCTIONS.md` rule for this component: any change adding or widening a public borrow-returning
  method answers M18.1's question in the PR. Both C-population defects were introduced by ordinary,
  well-reviewed changes; what was missing was the specific question, not the diligence.
  **Done:** [DESIGN-INSTRUCTIONS.md](DESIGN-INSTRUCTIONS.md) -- the repository's first, so it is kept tightly
  component-scoped. It states the question, what counts as answering it (three points, with the M18.1 audit's
  nineteen rows as worked examples), and that "no hole" is a complete answer that must still be written down.
  **A document alone would not have made this an obligation**, which is the item's own point: the rule it
  replaces was "remember to ask", and that failed three times. So the trigger is mechanical.
  [BORROW-SURFACE.txt](BORROW-SURFACE.txt) is a generated inventory of every public function in `src/` whose
  return type carries a borrow; `tools/check-borrow-surface.ps1` regenerates it and fails on any
  disagreement, wired into CI beside the existing `encoding` and `baseline` sanity jobs -- whose own header
  describes this same restatement-drift problem, so the pattern was already established here rather than
  invented.
  **Scope of the trigger, chosen deliberately:** a reference *or* a lifetime-carrying wrapper. A `&`-only
  scan would have missed `Batch<'_>` and `RingScope<'_>` -- which is precisely where M18.6's fix lives, so the
  narrower scan would have gone blind to the surface it was written for. Seven entries today.
  **Verified load-bearing:** adding `pub fn sabotage_ring(&mut self) -> &mut IoRing` -- D-43's exact shape --
  makes the check fail and print the question. It checks *shape*, not correctness, and the instructions say so
  plainly: it cannot tell a safe accessor from a dangerous one, and tuning it until it stops firing would be
  the failure mode to guard against.
  **Found while writing it:** `.Count` on a `Where-Object` pipeline is a `StrictMode` error when the result is
  empty or a single item -- the check crashed rather than passing on its first clean run. Fixed with `@(...)`.

- [x] **M18.3** -- Run `cargo-mutants` over the crate and triage the surviving mutants. There is a measured
  rate to justify it: this branch produced two vacuously-passing tests (the M11.2 idempotence pair, which
  asserted through a freshly attached event and so could not observe a detached one) and one `debug_assert`
  that could never fire (`Option::take` had already emptied the slot it checked). Both are exactly what a
  surviving mutant looks like.
  **Result:** 306 mutants in 32 minutes -- 182 caught, 48 missed, 7 timeouts, 69 unviable. Counting timeouts
  as detections (a mutant that hangs the suite fails CI as surely as one that trips an assertion), that is
  **189 of 237 viable mutants caught, 79.7%**. Triaged into six categories in
  [MUTATION-SURVIVORS.md](MUTATION-SURVIVORS.md), which is M18.4's input.
  **It found a third vacuous test, which four review rounds had read past.** Both `Arc<[u8]>` tests in
  `src/buf/tests.rs` compare `stable_ptr()` against *another call to the same function*, so returning a **null
  pointer** -- the buffer address this crate hands the kernel -- passes both. They assert self-consistency,
  never that the address is real. The neighbouring `&'static [u8]` test already shows the fix: compare against
  an independently obtained address.
  **And it caught the previous commit's author.** Every read-only accessor M18.6 added to `RingScope` survives,
  because no test calls any of them. The surface was added on the stated principle that a platform layer is
  not narrowed to its current caller, which stands -- but mutation testing showed within one commit that none
  of it is exercised.
  **A first reading of one survivor was wrong, and checking corrected it.** The `contract.rs` mutant deletes a
  match arm in `check_quiescent`, which looked like the oracle's own busy-buffer detection going untested. It
  is the **sort key**, not the detection: the test has one busy buffer, so ordering is unobservable. Recorded
  as equivalent-under-current-tests rather than as a hole.
  **Tooling, measured rather than assumed:** `cargo install cargo-mutants --locked` fails on this host --
  the locked `winapi` does not compile for `aarch64-pc-windows-msvc` (285 errors). Installing unlocked, from
  outside the repo so `rust-toolchain.toml`'s 1.98.0 pin does not apply to the tool, works. `--timeout 180` is
  needed because several mutants hang, and a cap derived from the ~5 s baseline is too tight for a suite whose
  own waits are 5 s. All of this is in the reproduction section of the survivors file.
  Note `cargo-mutants` has no `cargo_*` MCP tool, so it is the rare case where the terminal is the only route.
- [x] **M18.4** -- Resolve the survivors: strengthen the test, or record why the mutant is equivalent and
  harmless. Do not delete an assertion merely because a mutant survives it -- a dead assertion beside a live
  one is worse than none, which is why M14.3's was removed rather than repaired.
  **Result: 48 survivors down to 12, and the score from 79.7% to 94.9%** (219 caught, 12 missed, 6 timeouts,
  69 unviable of 306). 18 new unit tests; no production code changed. Verified by re-running `cargo-mutants`,
  not by assuming the tests would kill what they targeted -- which mattered, because four of them did not.
  **The vacuous `Arc<[u8]>` tests are fixed** by comparing `stable_ptr()` against `Arc::as_ptr`, an
  independently obtained address, exactly as the neighbouring `&'static [u8]` test already did.
  **Four tests failed to kill what they aimed at, and re-running found each one.** Worth recording, because
  every one of them looked correct: (1) the sort-order test used three busy buffers, and with the sort key
  deleted `sort_by_key` is stable, so `HashMap` order came out sorted anyway -- now eight; (2) `checked_index`
  is the *span* bounds check, not `get`'s, so an out-of-range `get` never reached it -- now submits a span;
  (3) `user_data() -> 0` matched because a fresh ring's first operation *is* zero -- now burns ids first, and
  then `-> 1` matched for the same reason, so it burns two; (4) `supports_raw` was asserted only in the
  negative, which a constant `false` satisfies. **A test written against a mutant is not verified until the
  mutant is re-run.**
  **Two survivors are provably unkillable**, argued rather than assumed: `as_hresult`'s `|` versus `^` operate
  on disjoint bit sets and are the same function; `with_injected_failure`'s zeroed `information` is
  unreachable because `result()` returns `Err` first. Recorded with proofs.
  **The rest are recorded with named reasons** in [MUTATION-SURVIVORS.md](MUTATION-SURVIVORS.md), separated
  into unkillable, host-blocked (two `supports -> true` mutants need an `Op` this host does not support, and
  widening a closed enum to satisfy a mutant would be inventing API for a test), queued (capability decoding,
  spawned as **M18.7**), and two `Debug` impls whose exact wording an assertion would only change-detect.
  "Hard to test" is written down as that, never as "equivalent".

- [x] **M18.7** -- Extract the capability flag decoding in `capabilities()` into a pure function over the raw
  `IORING_CAPABILITIES`, so it can be exercised with synthetic flag values. Spawned by M18.4: four mutants on
  `raw.FeatureFlags & FLAG != 0` survive because nothing can vary what `QueryIoRingCapabilities` returns, and
  the decoding of `supports_completion_event` in particular gates every completion-event path in the crate --
  [D-20](DESIGN-NOTES.md#d-20), and #47 lived downstream of it. A pure `fn decode(raw) -> Capabilities` plus a
  table of flag combinations kills all four and makes the one capability that matters testable in both
  directions, which no test can do today.
  **Done:** `decode(&IORING_CAPABILITIES) -> Capabilities` split out of `capabilities()`, which now does the
  syscall and nothing else. Pure refactor -- no behaviour change -- plus five tests, and a round-trip test
  tying `decode` back to the query so the split cannot drift.
  **All four mutants killed**, verified by a scoped re-run: `capability.rs` reports 12 mutants, 9 caught and
  3 unviable, none missed. The killing cases fall straight out of the pure function: an empty mask kills
  `& -> |` (because `flags | FLAG` is non-zero whatever `flags` holds), a mask carrying exactly one named flag
  kills `& -> ^` (which clears the very bit it is testing), and any positive case kills `!= -> ==`. An
  unknown-feature-bit case is included because this crate has already been surprised once by a Windows
  reporting a version it did not name.
  **Crate-wide: 307 mutants, 221 caught, 10 missed, 6 timeouts, 70 unviable -- 95.8%**, up from 79.7% before
  M18.4.
  **Recorded a caveat on the headline number rather than quoting it flat.** Two mutants -- `Batch::require`
  and `IoRing`'s `Debug` -- were reported caught in one run and missed in another *with identical sources*.
  The suite has timing-dependent tests and `cargo-mutants` runs four jobs in parallel, so a mutant can be
  "caught" by a test that merely flaked. Both are now filed as unresolved rather than fixed, and
  [MUTATION-SURVIVORS.md](MUTATION-SURVIVORS.md) says a single run's score is approximate.
  **`Batch::require -> Ok(())` was reclassified on the evidence:** `require` returns `Ok` exactly when
  `supports` is true, so on a host supporting every operation it is equivalent *in practice*. Filed under
  host-blocked rather than provably-unkillable, because the equivalence is a property of the host, not the
  code -- and its single "caught" result is consistent with the flake above.

- [x] **M18.5** -- Document the strategy as a whole in [DESIGN-NOTES.md](DESIGN-NOTES.md): the three defect
  populations, which technique covers which, and -- most importantly -- **what none of them cover.** Two of the
  eight defects came from spikes against the real kernel, and no oracle, generator or allocator would have
  produced either, because both were cases of Windows behaving differently from the assumed contract. Record
  spikes as a budgeted technique for each new Win32 surface rather than something that happens when a test
  mysteriously fails.
  **Done:** [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [Testing strategy](DESIGN-NOTES.md#testing-strategy-m185),
  plus [D-44](DESIGN-NOTES.md#d-44) making the spike rule citable from the decision index rather than reachable
  only through prose.
  **The item's own premise needed a correction, and checking found it.** "No allocator would have produced
  D-32" is not quite true: the guard allocator *does* catch it, as a hard `STATUS_ACCESS_VIOLATION`, measured
  in M17.4's calibration. The accurate statement is sharper and is what got written down -- every technique
  here checks this crate's code against **this crate's stated contract**, and in both spike-found defects the
  stated contract was the thing that was wrong. They detect a *consequence*, on a path some test already
  walks; a spike produces the platform knowledge that decides what the code should be, and is the only
  technique that runs **before there is code to test**.
  **Two things the write-up records that no single milestone did.** First, M15 and M16 finding nothing is not
  reassurance: they are passive, so zero findings meant the detectors had only seen the twenty-odd
  hand-written scenarios that already passed -- which is why M17 exists to feed them. Second, **every one of
  these instruments was wrong the first time and only sabotage found it**: M17.3's generator reported green
  with #47 reintroduced, four of M18.4's mutation-killing tests did not kill their mutant, and M15.2's poison
  inverse was fabricated twice. Budget the calibration, not just the instrument.
  Also records the two rejected techniques -- a mock `IoRing` and PageHeap -- so neither is re-proposed as an
  obvious win, and a per-technique table of what each actually found rather than what it was hoped to find.
- [x] **M18.6** -- Narrow `EventDelivery`'s ring surface so safe code cannot replace the ring out from under
  the pool's wait. Spawned by M18.1, recorded as [D-43](DESIGN-NOTES.md#d-43), and **measured**: the
  replacement compiled and silently stopped delivery -- one completion before the swap, none after.
  **The two obvious fixes do not work.** A `Deref`/`DerefMut` newtype still permits `*guard = ...`, and so
  does handing a `&mut IoRing` to a closure. Closing the hole means never letting a `&mut IoRing` escape at
  all.
  **Done:** `EventDelivery::ring -> &Mutex<IoRing>` is replaced by `EventDelivery::scope -> RingScope`.
  The exposure rule is stated rather than ad hoc: **every read-only part of `IoRing`, plus batch
  construction -- and nothing that can retarget the ring or steal the pool's completions.** So `batch`,
  `outstanding`, `info`, `version`, `supports`, `supports_raw` and the registered counts; deliberately no
  `try_pop` ([D-21](DESIGN-NOTES.md#d-21) makes the pool the single drainer), no `completion_event` (two
  waiters on one ring), no `run_down`, and no `&mut IoRing`.
  **Reviewed for sizing before starting, and one item was right.** Nine call sites across six files, all of
  which stop compiling the moment `ring()` is removed -- so any split would have produced a non-compiling
  intermediate commit, and the add-alongside-then-remove shape would have left the hole open across commits.
  Larger than the item text implied, though: it also forced `handover.rs`'s `submit_wave` to split into a
  `queue_wave` that fills a caller-owned `Batch` (four plain-ring call sites kept their old shape), because
  `Batch::submit_and_wait` consumes the batch and the scope hands out a `Batch` rather than a ring.
  **Proved by construction, not by review**, which is the whole point of M18: a `compile_fail` doctest
  asserts the replacement no longer compiles. That doctest was itself verified -- adding a `DerefMut` impl
  makes it **fail**, so it is passing for the right reason rather than on a typo in its own setup, and that
  same experiment is the empirical proof of the "a `Deref` newtype would not close this" claim above. It is
  paired with a normal doctest sharing the setup, since a `compile_fail` example passes on *any* error.
  **Incidental fix:** call sites were inconsistent about mutex poisoning -- some `expect`, some
  `unwrap_or_else(PoisonError::into_inner)`. `scope()` absorbs poisoning internally, matching what the wait
  callback's own drain already did, so the question no longer reaches callers at all.
  Verified: 148 tests pass, both affected examples (`model_a_delivery`, `epoch_log`) still run to exit 0.
