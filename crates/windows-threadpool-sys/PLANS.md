# Plans: windows-threadpool-sys

Completed checklists are recorded in
[COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they contained are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | Which milestones are open is what [CHECKLIST.md](CHECKLIST.md) says; `M-T8` was opened 2026-10-01 by `windows-ioring-sys`' `M27.1` census, which found thread placement has no expression in this workspace. `M-T7`: why a worker factory loses its worker. Transferred from `windows-ioring-sys` on 2026-10-01, with the timeline and 33 measurement captures, because `M26.13.4` established the fault is pool-wide and not that crate's. Diagnostic rather than remedial: the remedy shipped as M-T4's draining teardown and was re-measured at 0 in 12000. Open because `try_cancel_pending` remains public, so the hazard is still reachable, and the self-heal that mitigates it has never been measured against this stall. | [DESIGN-NOTES.md](../../DESIGN-NOTES.md#teardown-drains), [STALL-TIMELINE.md](STALL-TIMELINE.md) |
