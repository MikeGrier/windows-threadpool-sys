# Plans: windows-overlapped-io-sys

Completed checklists are recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they
contained are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M10: make `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` usable end to end -- an opt-in endpoint notification-mode setter, and a two-state `Started` submission outcome so the buffer-owning adapters can report a synchronous no-packet completion instead of rejecting it. Breaking by design, taken while the crate has no adopters. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
