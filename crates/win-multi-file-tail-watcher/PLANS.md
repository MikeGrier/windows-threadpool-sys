# Plans: win-multi-file-tail-watcher

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | not started | M1 register one file and read its length through an observer (needs windows-overlapped-io-sys M15 first); M2 a poll over many files reports attentions through the SQ; M3 full SQ, resume, and roster churn during a poll; M4 shutdown; M4+ reads on the thread pool, parked behind M4. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
