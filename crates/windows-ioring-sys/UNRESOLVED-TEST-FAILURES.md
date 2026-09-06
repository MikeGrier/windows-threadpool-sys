# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) (append-only) rather than deleting it.

## `flush_barrier::a_covering_flush_waits_for_preceding_writes_and_an_unordered_one_does_not`

**Flaky under a full-workspace run, green in isolation.** Observed once during
`cargo test --workspace --all-features` on 2026-09-03 (2899 of 2900 passed); the
same test re-run on its own with `--test flush_barrier` passes.

Not caused by the change that observed it, which touched
`windows-topology-sys` only. The test measures real I/O ordering, so it is
sensitive to load: a full workspace run has every other suite competing for the
disk, and the window this test asserts is a timing one.

Recorded rather than fixed because the failure mode -- a load-sensitive
assertion in a real-I/O test -- needs a decision about whether the test should
be made load-independent or marked as serial, and that is not this change's
scope. It has not been seen to fail in CI.
