# The fix, measured on the real `EventDelivery` path

2026-09-29. Closes **M-T4.6**.

## What this answers

Every earlier arm that moved the needle was a **hand-rolled model** of the
teardown, built from raw Win32 so its gap could be varied. That model is only
worth what its fidelity is worth, so the fix committed in `2affbb09`
(`feat(threadpool)!: teardown drains rather than cancels`) still had to be
measured on the path a caller actually takes: `EventDelivery`, built on this
crate's `ThreadpoolWait`.

## Arms

The same reproducer -- the three-test subset of
[event_delivery.rs](../../../windows-ioring-sys/tests/event_delivery.rs), run in a fresh process,
scored by exit code -- against two builds that differ only in `ThreadpoolWait`'s
teardown:

- **reverted** -- `target/reverted`, built with M-T4.2 undone, so `Drop` and
  `stop_and_drain` call `cancel_pending()` (`fCancelPendingCallbacks` TRUE).
- **current** -- `target/debug`, the committed build, where both call `wait()`
  (FALSE) and so drain.

Counts are in [arms.csv](arms.csv).

## Result

The reverted build reproduces. The drained build does not, over seven times the
runs.

Under the null hypothesis that draining leaves the rate alone, the reverted
build's rate predicts a count in the drained build's runs whose probability of
coming back zero is about 5e-101. The measurement does not distinguish
"eliminated" from "reduced below what 70000 runs can see"; what it excludes is
that the rate is unchanged.

## Provenance of the 60000 salvaged runs

Five of the nine blocks are labelled "salvaged arm N of the voided gap run", and
the label is load-bearing rather than decorative.

That run was *intended* to be five different teardown gaps. Its arms never
applied: the edit that was supposed to add them used a PowerShell `.Replace()`
whose anchor did not match the file, which returns the string unchanged and
reports nothing. The binary compiled, the runs completed, and all five arms ran
the identical committed trigger.

That voids the run for the question it was asked, and it is recorded in
[STALL-TIMELINE.md](../../STALL-TIMELINE.md) as a harness defect. But it does
not void the runs themselves: five arms of the identical committed path on the
identical build is exactly a 60000-run block of that path, and the failure of
the edit is *why* it is one block rather than five. It is reported here as five
rows rather than silently pooled so a reader can see the shape of what happened.

The lesson was taken the expensive way and is now standing practice: make the
edit with a tool that errors on a missing anchor, and verify the change in the
source before trusting a successful compile.

## What it does not say

Nothing here is a root cause. It says the committed change removes the failure
on the real path; it does not say why closing a wait handle behind its own
disarm stalls the pool in the first place. That question is still open -- see
[STALL-TIMELINE.md](../../STALL-TIMELINE.md).
