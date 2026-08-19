# Plans

Master tracker for every active CHECKLIST.md in the repository. Each source-component also keeps its own
PLANS.md: [crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md),
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md), and
[crates/windows-file-watcher/PLANS.md](crates/windows-file-watcher/PLANS.md). Checklists whose work is
finished move to [COMPLETED-PLANS.md](COMPLETED-PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [crates/windows-file-watcher/CHECKLIST.md](crates/windows-file-watcher/CHECKLIST.md) | not started | Memory-safe Windows path-change watcher over `ReadDirectoryChangesW` with a `FindFirstChangeNotification` coarse fallback: queue-mediated monitor/session/watch model, per-directory coalescing, a resident-policy autonomous fault machine with no terminal state, and the `Desync` re-scan primitive (M1 scaffold+decode → M7 docs/stress). | [crates/windows-file-watcher/design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](crates/windows-file-watcher/design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md) |

Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
