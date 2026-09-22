# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) (append-only) rather than deleting it.

## Observed 2026-09-21 21:34:00 -04:00 -- one unidentified failure in `tests/bounded_pop.rs`

**Seen once, not reproduced in 19 subsequent runs.** A `cargo test -p windows-ioring-sys
--all-features` run, immediately after a debug and a release `cargo check --all-targets`, reported
`FAILED. 4 passed; 1 failed` in a target finishing in 2.12s. That matches
[bounded_pop.rs](tests/bounded_pop.rs) -- five tests, 2.08-2.12s -- but the identification is by
shape, not by name.

**The failing test's name and panic message were not captured**, which is the defect in how this was
handled: the summary line was read and the run discarded. Re-running cannot recover it.

Not reproduced since, across: 15 consecutive isolated runs of `--test bounded_pop`, and 4
consecutive full `--all-features` suite runs. All green.

**The plausible mechanism, unconfirmed.** These tests need a read that is still pending when a short
bound expires, and they get it from a 128 MiB `FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED` read.
`NO_BUFFERING` bypasses the system cache but not the *device's* own, so a run where the drive serves
the read unusually fast would make `popped.is_none()` false. Each test asserts `outstanding() > 0`
beside it precisely so that case fails loudly rather than passing vacuously -- which is consistent
with what was seen, and is the assertion that would have fired.

**Why it is recorded rather than fixed.** The robust shape is an operation that *cannot* complete --
a read on an overlapped named pipe nobody writes to -- which removes the timing dependence entirely
rather than widening a margin. That needs `Win32_System_Pipes` added to the dev-dependency feature
set, and it needs confirming that `IoRing` will accept a pipe handle at all. Both are real work with
a real chance of not panning out, and neither belongs in a push of unrelated finished milestones.

Queued as `M22+.1` in [CHECKLIST.md](CHECKLIST.md).
