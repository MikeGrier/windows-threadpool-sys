# Resolved test failures: windows-threadpool-sys

Failures once recorded in [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md), moved here
when resolved. Append-only: newest at the bottom.

## Resolved 2026-10-08 01:28:24 -04:00 -- the forced-repair-failure tests raced on one slot

**Fix.** The single `FORCE_REPAIR_FAILURE_FOR` slot became a list of forced keys, and the guard that
writes it, `heal::ForcedRepairFailure`, moved into [heal.rs](src/heal.rs) beside it: each guard adds
its own entry and removes only that entry, once, whether by `lift` or on drop. One guard now serves
every forcing test, where two test modules had each written the slot their own way.

**A third test was in the race.** The record below names two tests, but
`a_release_whose_claim_pinned_nothing_reports_untracked_rather_than_re_registering` in
[cleanup_group/tests.rs](src/cleanup_group/tests.rs) wrote the same slot by hand, under
`--all-features`. It now uses the guard, lifting it early from its release hook.

**Regression test.** `overlapping_forced_failures_do_not_undo_each_other` sequences the overlap the
race needed: two pools forced, one forcing ended, and a second forcing of one pool lifted twice,
each followed by a check that the other forcing still holds, and finally that an unforced pool does
register. Its first version passed under the sabotages meant to break it: it dropped each
registration before checking, and a dropped registration's entry can retire at once, so "not
registered" held either way. Holding the registration across each check fixed that.

**Verification.** The self-heal tests ran 200 times in sequence with no failure, and the
all-features library binary 100 times. Three sabotages, each caught by the regression test: a
forcing that replaces the others, an ending that clears them all, and a lift that is not once-only;
and a control, removing an entry in order rather than by swap, survives. The crate's whole
sabotage manifest then ran as declared, the cleanup-group test that changed included.

### The record as it stood

**Recorded 2026-10-08 00:12:00 -04:00 -- the two forced-repair-failure tests race on one slot**

**Tests** (both in [heal/tests.rs](src/heal/tests.rs), module `heal::tests::on`):

- `a_cancellation_registers_a_pool_whose_first_registration_failed` -- fails with "the forced
  failure must leave the pool unregistered, or this test is not exercising the path it names".
- `a_cancellation_that_cannot_register_reports_the_pool_untracked` -- fails with "every allocation
  failed, so the cancellation cannot claim the pool is tracked".

**Symptom.** Intermittent, under the default parallel harness. Seen while verifying WT-2.1 of
[win-time-sys's CHECKLIST.md](../win-time-sys/CHECKLIST.md), running `<lib test binary> heal -q`
thirty times in sequence: two failures in thirty with that change, and one in thirty on the
unchanged tree before it -- so the failure predates the change, which does not touch either test's
path.

**Cause (argued from the code, not yet confirmed by a fix).** Both tests force a failure through
`ForcedRepairFailure`, which writes the pool key into the single process-wide
`heal::FORCE_REPAIR_FAILURE_FOR` slot and stores `0` on drop. Keying the slot to one pool stops a
forcing test from failing *another* test's registration, which is what its doc comment claims; it
does not stop the two forcing tests from interfering with *each other*. When they overlap, either
one's `store(key)` replaces the other's key, or one's `Drop` clears the slot while the other is
still inside its forced scope -- and in both cases the other's `register` succeeds, which is
exactly what each assertion reports.

**Not yet decided.** The fix is a change to the test seam (serialise the forcing tests, or make the
slot hold more than one key), which is outside WT-2.1 and is raised with the engineer rather than
folded into it.
