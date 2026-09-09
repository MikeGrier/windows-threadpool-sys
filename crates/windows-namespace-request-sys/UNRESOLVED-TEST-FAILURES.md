# Unresolved test failures: windows-namespace-request-sys

Observed failures that are real but not yet diagnosed. Recorded here rather than
left in a CI log, so a later intermittent failure is recognised as a known one
instead of being re-investigated from scratch or dismissed as noise.

When an entry is resolved, move it to a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) under a
`## Resolved <YYYY-MM-DD HH:MM:SS +hh:mm> -- <description>` heading in the same
change that removes it from this file. Do not delete entries.

## Observed 2026-09-09 15:35:14 -04:00 -- `close::tests` handle-value reuse poisons the allocation lock

**Symptom.** One CI run of `cargo test --workspace` reported **96 failures** in
this crate: `close::tests::a_caller_supplied_routine_is_carried` failed with
`assertion failed: !was_still_open(raw)`, and the other 95 all failed with
`the lock is not poisoned: PoisonError { .. }`. The 95 are collateral -- the
first test panicked while holding the write guard on `handle_allocation()`,
which poisons the lock for every test that takes it afterwards. **The count is
alarming and the defect is singular.**

**It is intermittent, and that is established rather than assumed.** The same
commit (`b90d897`) was re-run with no change and passed. The crate passes 5/5
locally. The two commits before it passed the identical job. The commit that
"failed" was comment-only, in a different crate.

**Mechanism, as far as it is understood.** `was_still_open` probes a raw value
by attempting to close it:

```rust
fn was_still_open(handle: HANDLE) -> bool {
    unsafe { CloseHandle(handle) != FALSE }
}
```

Its own comment states the precondition: "A stale value fails with
`ERROR_INVALID_HANDLE` rather than closing something else, **because these tests
hold the allocation lock**." That precondition does not hold for the whole
process. Measured on this revision: **218 tests in the crate, and 11 of them open
handles without taking `handle_allocation()`** -- among them
`open::tests::a_missing_path_reports_the_raw_code_unaltered`,
`open::tests::the_overlapped_flag_is_carried_rather_than_decided`, and
`watch::tests::a_missing_directory_reports_the_raw_code`.

`cargo test` runs tests as threads in one process (deliberately -- see the root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md)), so a handle value freed by a
lock-holding test can be immediately reallocated by one of those 11 running
concurrently. The probe then finds the value open and the assertion fails.

**The failing assertion is not the worst of it.** `was_still_open` *closes* the
handle when the probe succeeds. So in the losing interleaving this test does not
merely mis-report -- it closes a live handle belonging to another test, which
can surface later as an unrelated failure somewhere else entirely. The visible
assertion is the benign outcome.

**Not caused by the change that observed it.** The branch that hit this
(`mikegrier/probes-cost-pair`, PR #83) touches only
`crates/windows-platform-probes/`, `.github/workflows/ci.yml` and `Cargo.lock`;
it adds `windows-namespace-request-sys` as a *dependency* and changes none of its
code.

**Directions for whoever picks this up**, in rough order of directness:

1. Take the allocation lock in the 11 tests that open handles without it. This is
   the smallest change and closes the measured hole, but it leaves the invariant
   resting on every future test author remembering -- the same "a flat rule beats
   a rule someone must remember to apply" problem recorded in the root
   [DESIGN-NOTES.md](../../DESIGN-NOTES.md) for status checking.
2. Make the lock unnecessary by not probing a raw value at all. `was_still_open`
   exists to answer "did the close routine actually run?", and the close routines
   already have observation statics for that purpose. A probe that asks the
   routine rather than the OS cannot race.
3. Failing both, make the hazard structural: have `Fixture` / `captured_duplicate`
   take the read lock themselves, so opening a handle without the lock is not
   something a test can do by omission.

Direction 2 is the most promising and the largest; direction 1 would stop the
bleeding today.
