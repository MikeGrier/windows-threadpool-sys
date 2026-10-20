# Plans: windows-file-enumeration-sys

The crate's implementation is M5 through M7 in the workspace
[CHECKLIST.md](../../CHECKLIST.md). Completed plans are recorded in
[COMPLETED-PLANS.md](COMPLETED-PLANS.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md), with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | REVIEW-1: re-read against that contract, which no longer caps ordinary paths or rewrites any to `\\?\` form, and review it against a traversal layer before one is built. Raised from the `windows-file-watcher` side; schedules no change of its own. | [DESIGN-NOTES.md](DESIGN-NOTES.md), [workspace DESIGN-NOTES.md](../../DESIGN-NOTES.md#path-contracts-follow-path-construction) |
