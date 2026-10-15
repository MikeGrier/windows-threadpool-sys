# Plans: windows-overlapped-io-sys

Completed checklists are recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they
contained are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M14: finish auditing this crate's completion-delivery contract against the ten specification-gap categories. Five of the ten were reached while recording the taxonomy; M14.1 covers the remaining five (1, 2, 6, 8, 9) against the endpoint/association/submission surface, and M14.2 sweeps `outstanding()` for the `has_room` shape -- an advisory predicate a caller's reliability gate depends on. M16: more observer queries -- standard info, file ID, file type, storage and alignment (which the unbuffered adapters then ask for instead of assuming a page), remote protocol, basic info (measured first), final path, volume info -- and all of them against every backend with an operation pending. (M15, observers sharing an endpoint's handle, is complete and archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).) | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
