# Plans: windows-overlapped-io-sys

Completed checklists are recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they
contained are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M14: finish auditing this crate's completion-delivery contract against the ten specification-gap categories. Five of the ten were reached while recording the taxonomy; M14.1 covers the remaining five (1, 2, 6, 8, 9) against the endpoint/association/submission surface, and M14.2 sweeps `outstanding()` for the `has_room` shape -- an advisory predicate a caller's reliability gate depends on. M15: endpoints share their handle with query-only observers (M15.1 decided; M15.2-M15.5 build it, including windows-threadpool-sys' `ThreadpoolIo`; M15.6 keeps an observer's query from disturbing `BlockingEndpoint`'s wait); requested by win-multi-file-tail-watcher. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
