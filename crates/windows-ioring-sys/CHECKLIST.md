# Checklist: windows-ioring-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-22-ioring-architecture.md](design-sessions/DESIGN-SESSION-2026-08-22-ioring-architecture.md).
Completed milestones are archived in COMPLETED-CHECKLIST.md once there are any.

## M1 -- Ring lifecycle and capability negotiation

- [x] **M1.1** -- Create the crate skeleton: `Cargo.toml`, `src/lib.rs`, README, and add it to the
  workspace `members`. `windows-sys` features are `Win32_Foundation`, `Win32_System_IO`, and
  `Win32_Storage_FileSystem` (which is where the IoRing bindings live). Everything is behind
  `cfg(windows)`, as elsewhere in this repository.

- [ ] **M1.2** -- A typed owner for `HIORING` whose destructor is `CloseIoRing`, per the repository rule
  that a resource with a specialized destructor gets its own owner rather than a generic handle wrapper.
  Not `Clone`.

- [ ] **M1.3** -- Capability query and version negotiation (D-6). `QueryIoRingCapabilities` needs no ring,
  so expose it as a free function for consumers deciding whether to use the crate at all. Ring creation
  negotiates `min(highest we understand, caps.MaxVersion)`, stores it, and exposes it. Surface
  `UM_EMULATION` rather than hiding it: a consumer reaching for this crate to maximize throughput needs to
  know the ring is emulated.

- [ ] **M1.4** -- Probe every op once at construction into a cached capability set, plus
  `supports_raw(op_code)` for ops the OS has and this crate has not wrapped (D-7). The public op enum is
  `#[non_exhaustive]`.

- [ ] **M1.5** -- Integration test: create and close rings at each version the machine supports, assert
  capability reporting is self-consistent (a ring never claims an op its capability set denies), and assert
  a ring created at a negotiated version reports that version back through `GetIoRingInfo`.

## M2 -- Operation identity, buffer ownership, and rundown

- [ ] **M2.1** -- Duplicate `IoBuf`/`IoBufMut` from `windows-overlapped-io-sys` (D-1), with the contract
  extended to cover registration outliving a single operation. Record the extension in DESIGN-NOTES; do
  **not** extract a shared crate yet -- that decision is M6+.

- [ ] **M2.2** -- `Token<B>` owning its buffer, with a generation-stamped `usize` as the `UserData` carried
  through the SQE and returned in the CQE (D-4). Validation on completion rejects a token that does not
  match the generation, so a stale token cannot claim a later operation's completion -- the same guarantee
  `OperationId` makes in `windows-overlapped-io-sys`, but cheaper here because `UserData` is ours to choose
  rather than being an address we have to stamp separately.

- [ ] **M2.3** -- `Drop` on an uncompleted token forgets its buffer rather than freeing it (D-4). Test that
  the leak happens and that nothing is freed, because a use-after-free here would be silent and remote.

- [ ] **M2.4** -- In-flight accounting and rundown: `CloseIoRing` must not run with operations outstanding.
  Mirror the rundown discipline `windows-overlapped-io-sys` already uses, including its bounded, rechecked
  wait rather than an unbounded one.

## M3 -- The submission builder

- [ ] **M3.1** -- `Batch` holding `&mut IoRing`, submitting on drop, with `submit()` consuming it and
  returning the submitted count (D-5). Document loudly that the submission queue is ring state, so a
  dropped batch still submits -- the alternative would strand SQEs whose buffers a later unrelated submit
  would hand to the kernel.

- [ ] **M3.2** -- Per-op builders reached from the batch, each returning a token from `push()`:
  `read`, `write`, `flush`, `cancel`. Options chain (`offset`, `drain_preceding` for
  `IOSQE_FLAGS_DRAIN_PRECEDING_OPS`) so the common case stays short and the barrier stays discoverable.
  Every method checks its op's capability bit from M1.4.

- [ ] **M3.3** -- Submission-queue backpressure: `push()` surfaces `IORING_E_SUBMISSION_QUEUE_FULL`
  (`0x80460002`, observed at exactly entry 64 on a 64-entry queue) as a distinguishable error rather than
  auto-flushing. Auto-flush would silently change submission ordering and timing.

- [ ] **M3.4** -- `submit_and_wait(n, timeout)` exposing the fused form, which is the primitive Model B is
  built on (D-3) and not merely a convenience.

- [ ] **M3.5** -- The narrow unsafe raw-SQE seam (D-7), documented with the same framing as the `device`
  family's unsafe `ioctl`: it exists so a consumer is not blocked on us wrapping a new op.

- [ ] **M3.6** -- Integration tests: a batch of many reads round-trips with every `UserData` preserved and
  every buffer returned; a deliberately overfilled queue reports backpressure at the right entry and stays
  usable afterwards; a dropped batch still submits; a cancel of a target that is not outstanding reports
  `ERROR_NOT_FOUND` through the completion rather than at build time.

## M4 -- Model A delivery: completion event and the thread pool

- [ ] **M4.1** -- Refuse to construct the event-driven path when `SET_COMPLETION_EVENT` is absent, with
  `io::ErrorKind::Unsupported`, rather than silently degrading into a thread-based loop. Behavior is owned,
  not inherited: a consumer who asked for threadless delivery and got a thread has been told something
  false. The threaded path is a separate, explicit choice (M6+).

- [ ] **M4.2** -- Wire `SetIoRingCompletionEvent` to a `ThreadpoolWait` from `windows-threadpool-sys`,
  using an auto-reset `WaitableHandle::event`. The drain discipline is drain-to-`S_FALSE`, re-arm, drain
  again: the spike showed the event is set when completions land and auto-resets on wait, so the worst case
  is a harmless spurious callback, but the ordering is what makes a completion arriving between the last
  pop and the re-arm impossible to lose.

- [ ] **M4.3** -- Teardown: quiesce the wait, run down outstanding operations, then close the ring, in that
  order. Interaction with `CleanupGroup` documented, since a consumer will reasonably expect to put this
  in one.

- [ ] **M4.4** -- Integration test: submit with `wait_operations = 0` and assert completions are delivered
  on pool threads without the submitting thread ever waiting; assert teardown with operations in flight
  neither hangs nor closes the ring early.

## M5 -- Registration

- [ ] **M5.1** -- Registered file handles: a typestate carrying the registered index, so a `read` against
  a registered file cannot be written against an unregistered one by mistake.

- [ ] **M5.2** -- Registered buffers, with the ownership rule that registration outlives any single
  operation (M2.1) and the pinning cost documented, because it is the axis that punishes over-sharding
  (D-8).

- [ ] **M5.3** -- Integration test: a read addressing both a registered file index and a registered buffer
  index round-trips; a registration outliving many operations stays valid; dropping the registration while
  operations are in flight is refused rather than permitted.

## M6 -- Documentation for consumers, and the guidance that motivated this crate

- [ ] **M6.1** -- Render the "Two delivery architectures" material from [DESIGN-NOTES.md](DESIGN-NOTES.md)
  as crate-level rustdoc and README content, not only as a maintainer-facing design note. Consumers
  reaching for this crate are trying to maximize I/O throughput; the Model A / Model B trade-off, the
  reason the NUMA node is the wrong partitioning key, and the observation that buffer placement likely
  dominates thread placement are the things they need and cannot easily derive.

- [ ] **M6.2** -- A worked example of Model A: threadless delivery through the pool, which is the shape most
  consumers should start with.

- [ ] **M6.3** -- Topology guidance as documentation rather than as API (D-8): how to enumerate L3 domains
  with `GetLogicalProcessorInformationEx`, why processor groups are a hard floor, and how to allocate a
  node-local pool with `VirtualAllocExNuma`. Pointers, not a partitioning policy.

## M7 -- `ring-copy`: a topology-aligned sample

> **-> CROSS-COMPONENT PREREQUISITE:** blocked on component `crates/windows-topology-sys` ->
> `M4` (safe enumeration, the description, and its serialization). See
> [../windows-topology-sys/CHECKLIST.md](../windows-topology-sys/CHECKLIST.md).

This is a **sample**, not library surface. The library still owns no partitioning policy (D-8); the sample
is where a policy lives, so that the guidance in M6 has something executable behind it.
`windows-ioring-sys` itself does **not** depend on `windows-topology-sys` -- only the sample does.

- [ ] **M7.1** -- The `Topology -> Policy -> RingPlan` pipeline, with `Policy` as named code (`ByL3`,
  `ByNode`, `ByPackage`, `ByCore`, `Single`) rather than data. A plan names, per domain, the ring to
  create, the processors to affinitize to, and where its buffer pool should be allocated.

- [ ] **M7.2** -- Reject a plan the platform cannot express, rather than emitting an impossible affinity
  mask: a fed-in description may carry one group with more than 64 processors, which is legal in the
  description and unrepresentable on Windows (topology D-10).

- [ ] **M7.3** -- The copy engine itself: read and write through per-domain rings, buffers allocated with
  `VirtualAllocExNuma` and registered once per ring.

- [ ] **M7.4** -- Switches for topology source (discovered, or a JSON file), policy, and **buffer
  placement** (node-local versus deliberately remote). Buffer placement is the variable most likely to
  show a real effect, because the device DMAs into the registered buffer on every operation, whereas
  callback placement is a one-time cache-warmth question.

- [ ] **M7.5** -- Report per-domain throughput, and **say plainly when the host cannot show a
  difference** -- a single-node or virtualized machine will produce noise, and a benchmark that reports
  noise as a result is worse than no benchmark. The machine this was designed on reported zero NUMA nodes.

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
