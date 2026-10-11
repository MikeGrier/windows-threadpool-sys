# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) (append-only) rather than deleting it.

Earlier entries have been moved to [RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md),
which records what each entry was, when it was closed, and what closed it.

That history is deliberately not summarised here. A summary is a second copy of what the
resolved file already owns, and it goes stale without anything noticing -- the sentence
this one replaces named the wrong incident entirely, describing a stall resolved on one
date while the entry actually moved was a different failure found a week earlier.

## Recorded 2026-10-11 18:03:39 -04:00 -- the ring-opening lib-test inventory check fails

`tools/check-ring-tests.ps1`, which CI runs, reports five `completion_wake::` lib tests as
`ADDED` to the inventory in [RING-OPENING-LIB-TESTS.txt](RING-OPENING-LIB-TESTS.txt). They
came with `CompletionWake` in `M33.1` (commit `f80ba6d7`), which added them without answering
the check's question -- does each test need the kernel, or only a ring-shaped thing? Found
while landing `M35.1`, which adds no lib test that opens a ring.

What the answer looks like, not yet acted on: the five use only public API
(`CompletionWake`, `Batch`, `IoRing`, `PushOptions`, `SharedHandle`), which the check's own
guidance says moves them to `tests/` (`M24.3`) rather than into the inventory.
