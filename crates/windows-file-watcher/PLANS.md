# Plans: windows-file-watcher

Active planned work for the crate. Completed checklists move to `COMPLETED-PLANS.md`, and their milestones
archive to `COMPLETED-CHECKLIST.md`, both in this directory.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | not started | Memory-safe Windows path-change watcher over `ReadDirectoryChangesW` with a `FindFirstChangeNotification` coarse fallback: a queue-mediated monitor/session/watch model, per-directory coalescing, a resident-policy autonomous fault machine with no terminal state, and the `Desync` re-scan primitive. Seven milestones (M1 scaffold+decode → M7 docs/stress). | [design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md) |
