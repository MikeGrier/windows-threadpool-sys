# Checklist: windows-ioring-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-22-ioring-architecture.md](design-sessions/DESIGN-SESSION-2026-08-22-ioring-architecture.md).
M1 through M6 (ring lifecycle through consumer documentation) are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-22----m1-through-m6-ring-lifecycle-through-consumer-documentation).

## M7 -- `ring-copy`: a topology-aligned sample

> **-> CROSS-COMPONENT PREREQUISITE (satisfied):** was blocked on component
> `crates/windows-topology-sys` -> `M4` (safe enumeration, the description, and its
> serialization), completed 2026-08-22. See
> [../windows-topology-sys/COMPLETED-CHECKLIST.md](../windows-topology-sys/COMPLETED-CHECKLIST.md#moved-2026-08-22----m1-through-m4-safe-enumeration-the-description-serialization-and-documentation).

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
