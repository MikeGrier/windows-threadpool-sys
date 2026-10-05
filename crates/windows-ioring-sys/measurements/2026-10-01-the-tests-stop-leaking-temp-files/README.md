# The tests stop leaking temp files -- 2026-10-01

Closes **M26.14**. Counts in [arms.csv](arms.csv).

## The item's premise was wrong, and the correction is what shaped the fix

`M26.14` said **15** of this crate's test files build paths under the temp
directory and **none clean up**. The first half is right; the second is not.
Twelve of the fifteen already call `std::fs::remove_file` at the end of the test
body -- 45 call sites -- and `flush_barrier_stress.rs` already had a correct RAII
guard with the hazard written down.

The real defect is narrower and explains the observed leak better: **a trailing
`remove_file` does not run when the test panics**, and the `M26.13`
investigation ran, many times over, a reproducer whose failing arm panics --
see [its record](../../../windows-threadpool-sys/measurements/2026-09-28-re-verifying-the-premises/README.md#a-defect-the-check-produced)
for how many runs, and how much they left behind.
Three files -- `completion_event.rs`, `event_delivery.rs` and
`submission_lifecycle.rs` -- also had no removal at all, on any path.

## Measured

An all-passing run of this crate's suite was leaking, which is a smaller claim
than the investigation's own total but a checkable one. The temp directory was
cleared, the suite run, and the files matching this crate's prefix counted.

An all-passing run left temp files behind before the fix, and left none after
it. The counts are in [arms.csv](arms.csv), which is the authoritative capture
for this measurement and is where they stay: a figure typed into prose beside
the data it came from is a second copy somebody has to keep true by hand, and
nothing checks that they still agree.

The files already present when this started are the residue of earlier work, and
were cleared rather than counted.

## The panicking path, which is the one that mattered

A clean run on the passing path does not establish the property the fix exists
for.
`completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting`
was given a deliberate `panic!` placed **after its file handle is open**. The
run panicked at that line, exited 101, and left nothing behind.
[arms.csv](arms.csv) carries that arm and its count, for the reason given above.

That site previously had no removal on any path, so the same panic leaked before
the change.

What this arm shows is that the guard runs on the panicking path. It does
**not** show that a removal survives an open handle that refuses deletion: this
test opens its file with `std::fs::OpenOptions`, which shares delete, so the
removal could succeed with the handle still open (see the next section). An
earlier version of this paragraph called that state the one "in which a removal
can fail outright", which is true only of the direct `CreateFileW` fixtures; no
panic arm was run against one of those (PR #113 review).

## Deleting a file whose handle is open: it depends on how the file was opened

An earlier version of this section said every converted site depended on
declaration order. That was true of only some of them (PR #113 review). The
authoritative description of the two kinds of open is on `TempPath` in
[tests/common/mod.rs](../../tests/common/mod.rs); in brief:

- A fixture opened with `std::fs::OpenOptions` shares delete, so its guard can
  remove the file while the handle is still open, and drop order plays no part.
- A fixture opened directly with `CreateFileW` and no `FILE_SHARE_DELETE`
  cannot be removed while the handle is open -- the lesson
  `flush_barrier_stress.rs` records, where holding the handle in the same
  struct meant every trial silently leaked a 32 MiB extent. For those, the
  guard must outlive the handle, and since locals drop in reverse declaration
  order, it is declared **before** the handle. The converted sites of this kind
  already declared `let path = ...` ahead of `let file = ...`; the guard
  documents the requirement so a new site does not get it wrong by accident.

## What was deliberately not changed

**The 45 existing `remove_file` calls stay.** They are not redundant in the way
they first appear: each runs at a point the test controls, *after* the test has
closed its own handle, which is strictly more reliable than a drop order that
depends on how the locals were declared. The guard is the net underneath them
for the panicking path, and a removal that finds nothing is not an error, so the
two compose.

**`flush_barrier_stress.rs` keeps its own `Fixture` guard.** It already closes
the handle before deleting, in a `Drop` written around that hazard. Replacing
working, hard-won code for the sake of uniformity would have risked the exact
defect it was written to fix.

## What this does not say

It does not establish a rate for the leak under failure, only that the three
unguarded files leaked unconditionally and that the guarded path now survives a
panic. The size of the leak the investigation saw is in
[its own record](../../../windows-threadpool-sys/measurements/2026-09-28-re-verifying-the-premises/README.md#a-defect-the-check-produced),
which owns that figure; it is not re-derived, or repeated, here.
