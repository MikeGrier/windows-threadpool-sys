# Plans: windows-ioring-sys

Completed checklists are recorded in COMPLETED-PLANS.md once there are any, and the milestones they
contained are archived in COMPLETED-CHECKLIST.md. Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | not started | Memory-safe Rust over the Windows 11 / Server 2022 `IoRing` submission/completion ring, as a separate crate from `windows-overlapped-io-sys` (duplicate-then-decide). Covers ring lifecycle and capability negotiation, zero-allocation token-owned buffers, the batch submission builder, and threadless delivery through `ThreadpoolWait`. The pinned-thread (Model B) architecture is scoped as parked `M6+` by the engineer's direction. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
