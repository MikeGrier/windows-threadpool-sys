# Plans: windows-threadpool-sys

Completed checklists are recorded in
[COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they contained are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M17: a custom-close owner so `ThreadpoolWait` can own a wait target closed with a caller-supplied routine (e.g. `FindCloseChangeNotification`) instead of `CloseHandle` -- prerequisite for the windows-file-watcher coarse fallback (its M6.1). | [../../DESIGN-NOTES.md](../../DESIGN-NOTES.md) |
