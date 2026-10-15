# Plans: win-multi-file-tail-watcher

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | not started | M1 register one file and read its length through an observer (on windows-overlapped-io-sys' observers, its M15); M2 a poll over many files reports attentions through the SQ; M3 full SQ, resume, and roster churn during a poll; M4 shutdown; M4+ reads on the thread pool, parked behind M4. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
