# Plans: windows-ioring-sys

Completed checklists are recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md) once there are any, and the milestones they
contained are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | Memory-safe Rust over the Windows 11 / Server 2022 `IoRing` submission/completion ring, as a separate crate from `windows-overlapped-io-sys` (duplicate-then-decide). Covers ring lifecycle and capability negotiation, zero-allocation token-owned buffers, the batch submission builder, threadless delivery through `ThreadpoolWait`, file/buffer registration, consumer-facing documentation, and the `ring-copy` topology-aligned sample (M1-M7 archived). The pinned-thread (Model B) architecture remains parked as `M6+` by the engineer's explicit direction. `M8` (complete) closed a PR #20 review finding: `FileRef::Raw(HANDLE)`'s lifetime gap, fixed with `unsafe fn` raw entry points plus a safe, `Arc<OwnedHandle>`-backed `SharedFile` wrapper for the common case. `M9` (complete) closed further PR #20 review findings: cross-ring `Token`/`RegisteredFile`/`RegisteredBuffers` confusion (a new per-ring `RingId`, checked at claim/push time), `PendingBufferRegistration` freeing its buffers instead of leaking them on an unclaimed drop, and `Batch::do_submit` letting `Drop` silently retry an already-attempted, already-failed submit. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
