# Unresolved test failures: windows-file-watcher

Observed failures that are real but not yet diagnosed. Recorded here rather than
left in a CI log, so a later intermittent failure is recognised as a known one
instead of being re-investigated from scratch or dismissed as noise.

When an entry is resolved, move it to a sibling `RESOLVED-TEST-FAILURES.md`
under a `## Resolved <YYYY-MM-DD HH:MM:SS +hh:mm> -- <description>` heading in
the same change that removes it from this file. Do not delete entries.

## `watcher::tests::a_dropped_watcher_stops_delivering` -- intermittent on CI

**First observed:** 2026-08-29, PR #46, run 33225948225 (job 99029623588).

**Symptom:** the assertion at
[src/watcher/tests.rs](src/watcher/tests.rs) fails --

```
a dropped watcher must deliver nothing further
```

The test creates a file, waits for its notification, drops the watcher, records
the settled notification count, creates a second file, sleeps 200 ms, and
asserts the count is unchanged.

**Why it is recorded rather than fixed:** the failure appeared on a branch that
does not touch this crate, in a commit that changed only a Markdown design note.
An immediate re-run of the same job passed, and the test does not reproduce
locally: 20 isolated runs and 6 full-suite runs, all clean. So it is
environment- or load-sensitive rather than a regression, and the cause is not
yet known.

**What the failure would mean if it is real:** the test's premise is that
dropping a watcher completes rundown, so no further callback can run and the
count is settled *before* the second file is created. A failure says something
was delivered after that point, which would mean either that rundown had not
actually completed when `drop` returned, or that a callback outlived it. Both
would be genuine defects in the rundown path rather than test-only problems, so
this should not be written off as flakiness without establishing which.

**Suggested next step:** rather than adding a longer sleep -- which would hide
the failure rather than explain it -- have the test observe rundown completion
directly, so "the count is settled" is a checked fact rather than an assumption
about timing.
