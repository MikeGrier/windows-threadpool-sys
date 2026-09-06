# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) (append-only) rather than deleting it.

## `flush_barrier::a_covering_flush_waits_for_preceding_writes_and_an_unordered_one_does_not`

**Flaky under a full-workspace run, green in isolation.** Seen twice:

| When | Run | Result |
|---|---|---|
| 2026-09-03 | `cargo test --workspace --all-features` | 2899 of 2900 passed |
| 2026-09-06 | `cargo test` after a merge from `main` | 978 of 979 passed, `left: 1, right: 0` -- one of 32 writes completed ahead of the covering flush |

Neither was caused by the change that observed it. The 09-03 run's change
touched `windows-topology-sys` only; the 09-06 run was a merge that did not
touch `tests/flush_barrier.rs` or `src/batch.rs` at all.

The test measures real I/O ordering, so it is sensitive to load: a full
workspace run has every other suite competing for the disk, and the window this
test asserts is a timing one. Measured after the second failure -- **ten
consecutive isolated runs passed, and the full suite passed on immediate
re-run** (2913 of 2913), so the failure follows contention rather than any
state the test leaves behind.

Recorded rather than fixed because the failure mode -- a load-sensitive
assertion in a real-I/O test -- needs a decision about whether the test should
be made load-independent or marked as serial, and neither observing change had
that in scope. It has not been seen to fail in CI.

**A second sighting in three days is worth reading as a rate rather than an
anomaly.** Both were local full-workspace runs, which is where the contention
is; if that rate holds it will eventually land in CI, where it would read as a
real ordering defect to whoever sees it first.
