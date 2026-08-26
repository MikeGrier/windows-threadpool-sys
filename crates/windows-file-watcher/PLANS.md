# Plans: windows-file-watcher

Active planned work for the crate. Completed checklists are archived in
[COMPLETED-PLANS.md](COMPLETED-PLANS.md), and their milestones in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

All milestones through M12 (the PR #20 review response: M10's `FailureCode`/`OpenFailure` detail, M11's
reopen-identity fix, M12's per-subscription volume-change confirmation) are complete. M13 (the consumer
test surface) is the active post-v1 line of work. [CHECKLIST.md](CHECKLIST.md) also retains its parked
`M-inf` horizon bucket: work placed outside v1 by a recorded design decision, holding nothing pending.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) -> M13 | in progress | Consumer test surface: an off-by-default `test-util` feature, docs, and an example letting a downstream consumer drive its own notification-handling code with synthetic notifications through the real `Receiver` (no filesystem, no thread pool). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
