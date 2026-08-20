# Plans: windows-file-watcher

Active planned work for the crate. Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md); when the whole checklist completes, its entry moves to a
sibling completed-plans tracker created in this directory at that time.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | Memory-safe Windows path-change watcher over `ReadDirectoryChangesW` with a `FindFirstChangeNotification` coarse fallback: a queue-mediated monitor/session/watch model, per-directory coalescing, a resident-policy autonomous fault machine with no terminal state, and the `Desync` re-scan primitive. Eight milestones (M1 scaffold+decode -> M8 wtf-string adoption). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
