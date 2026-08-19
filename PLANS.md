# Plans

Master tracker for every active CHECKLIST.md in the repository. Each source-component also keeps its own
PLANS.md: [crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md),
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md), and
[crates/windows-file-watcher/PLANS.md](crates/windows-file-watcher/PLANS.md). Checklists whose work is
finished move to [COMPLETED-PLANS.md](COMPLETED-PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [crates/windows-file-watcher/CHECKLIST.md](crates/windows-file-watcher/CHECKLIST.md) | in progress | Memory-safe Windows path-change watcher over `ReadDirectoryChangesW` with a `FindFirstChangeNotification` coarse fallback: queue-mediated monitor/session/watch model, per-directory coalescing, a resident-policy autonomous fault machine with no terminal state, and the `Desync` re-scan primitive (M1 scaffold+decode → M7 docs/stress). | [crates/windows-file-watcher/DESIGN-NOTES.md](crates/windows-file-watcher/DESIGN-NOTES.md) |
| [crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md) | in progress | M17: custom-close wait owner so `ThreadpoolWait` can own a wait target closed with a caller-supplied routine (e.g. `FindCloseChangeNotification`) instead of `CloseHandle` — prerequisite for the windows-file-watcher coarse fallback (M6.1). | [DESIGN-NOTES.md](DESIGN-NOTES.md) || [crates/wtf-string/CHECKLIST.md](crates/wtf-string/CHECKLIST.md) | in progress | `OsString`-shaped strings with native `u16` (WTF-16), conversion-free storage for Windows FFI: an encoding-generic core (`WtfString<E>`) shipping the `Wtf16` arm, always-terminated storage, portable `str`/`String` conversions, and Windows-only lossless `OsStr` interop. Six milestones (M1 → M6). | [crates/wtf-string/DESIGN-NOTES.md](crates/wtf-string/DESIGN-NOTES.md) |
Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
