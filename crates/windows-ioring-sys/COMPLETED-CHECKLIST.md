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
