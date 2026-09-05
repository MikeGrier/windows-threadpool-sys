# Plans: windows-file-enumeration-sys

The crate's implementation is M5 through M7 in the workspace
[CHECKLIST.md](../../CHECKLIST.md). Completed plans are recorded in
[COMPLETED-PLANS.md](COMPLETED-PLANS.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md), with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | not started | REVIEW-1: review the request path contract against a traversal layer before one is built -- whether the deliberate `MAX_PATH` cap on ordinary paths survives descent, and whether moving into `\\?\` form mid-descent is specified for every namespace the crate accepts. Raised from the `windows-file-watcher` side; schedules no change of its own. | [DESIGN-NOTES.md](DESIGN-NOTES.md), [workspace DESIGN-NOTES.md](../../DESIGN-NOTES.md#path-contracts-follow-path-construction) |
