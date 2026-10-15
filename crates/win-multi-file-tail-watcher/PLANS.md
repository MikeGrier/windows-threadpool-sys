# Plans: win-multi-file-tail-watcher

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M1 register one file and read its length through an observer (on windows-overlapped-io-sys' observers, its M16); M2 the poll engine, three passes and a stop point, driven synchronously and benchmarked; M3 the poll on the thread pool, parking on a full SQ and resuming on the caller's doorbell, with roster churn; M4 shutdown; M4+ reads on the thread pool, parked behind M4. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
