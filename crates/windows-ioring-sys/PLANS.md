# Plans: windows-ioring-sys

Completed checklists are recorded in COMPLETED-PLANS.md once there are any, and the milestones they
contained are archived in COMPLETED-CHECKLIST.md. Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | Memory-safe Rust over the Windows 11 / Server 2022 `IoRing` submission/completion ring, as a separate crate from `windows-overlapped-io-sys` (duplicate-then-decide). Covers ring lifecycle and capability negotiation, zero-allocation token-owned buffers, the batch submission builder, threadless delivery through `ThreadpoolWait`, file/buffer registration, consumer-facing documentation, and the `ring-copy` topology-aligned sample (M1-M7 archived). Only the pinned-thread (Model B) architecture remains, parked as `M6+` by the engineer's explicit direction. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
