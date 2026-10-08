# Unresolved test failures: windows-threadpool-sys

Observed failures that are real but not yet fixed. Recorded here rather than left in a terminal,
so a later intermittent failure is recognised as a known one instead of being re-investigated from
scratch or dismissed as noise.

When an entry is resolved, move it to a sibling `RESOLVED-TEST-FAILURES.md` (created by the first
such move) under a `## Resolved <YYYY-MM-DD HH:MM:SS +hh:mm> -- <description>` heading, in the same
change that removes it from this file. Do not delete entries.

## Recorded 2026-10-08 00:12:00 -04:00 -- the two forced-repair-failure tests race on one slot

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
