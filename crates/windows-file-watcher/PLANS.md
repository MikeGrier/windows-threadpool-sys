# Plans: windows-file-watcher

Active planned work for the crate. Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md); when the whole checklist completes, its entry moves to a
sibling completed-plans tracker created in this directory at that time.

All milestones through M9+ (concurrency/spoilers/queue overwhelm) are complete. M10, M11, and M12 are a
new active plan, opened in response to a PR #20 review: M10 surfaces the real failure detail behind a
fault; M11/M12 make a reopen notice and let a client confirm when it lands on a different volume than
before. [CHECKLIST.md](CHECKLIST.md) also retains its parked `M-inf` horizon bucket: work placed outside
v1 by a recorded design decision, holding nothing pending.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M10: give a client the real `FailureCode`/`OpenFailure` behind a fault or permanent stop instead of only which operation faulted (D-79, supersedes D-54). M11: `WatcherInner::reopen` tries `ReOpenFile` against its still-live previous handle before falling back to a path-based open, and fixes a stale `DirectoryId` map key. M12: an opt-in per-subscription confirmation when a reopen lands on a different volume (D-78). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
