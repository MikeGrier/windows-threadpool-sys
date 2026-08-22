# Completed plans: windows-file-watcher

Append-only archive of completed checklists, moved out of [PLANS.md](PLANS.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-21 | Memory-safe Windows path-change watcher over `ReadDirectoryChangesW`, with a `FindFirstChangeNotification` coarse fallback. All eight planned milestones landed (M1 scaffold+decode -> M8 wtf-string adoption): a queue-mediated monitor/session/watch model with no client callbacks on the cadence path, per-directory coalescing with file (path) targets, an autonomous fault machine with no terminal state and a D-27 interactive retry protocol, the detailed/coarse two-tier fallback, a crate README and runnable examples, and adoption of `wtf-string` for conversion-free relative names. Decisions D-1...D-65 are recorded. The checklist file remains for its parked `M-inf` horizon bucket, which holds no pending work. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
