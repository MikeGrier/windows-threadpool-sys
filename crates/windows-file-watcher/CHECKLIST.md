# Checklist: windows-file-watcher

Memory-safe Windows path-change watcher. The design session that opened the crate recorded D-1...D-20 in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md).
The authoritative Tier-1 set is [DESIGN-NOTES.md](DESIGN-NOTES.md), which now runs to **D-80** -- later
decisions (D-21 from M1 review, D-22...D-26 and D-34/D-35 from M2, D-36...D-49 from M3, D-50...D-52 from M4,
D-53...D-59 from M5, D-60...D-65 from M6, D-32 from M8.1, D-66...D-76 from M9.1...M9+.4, D-25/D-27...D-31
plus D-33 from the [2026-08-21 fault-protocol session](design-sessions/DESIGN-SESSION-2026-08-21-fault-protocol-and-doorbells.md)
(which **overturned D-16**), and D-78/D-79/D-80 from the PR #20 review response and M11's own execution,
D-79 superseding D-54, D-80 revising M11.2's own reopen mechanism) are added there as milestones complete.

Work items are dependency-ordered. Each milestone ends with integration tests. The implicit
end-of-milestone gate (default **and** `--all-features` build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

> **NEXT ACTIONABLE ITEM: none.** M1 through M14 are archived/done. Only the parked, ungated M-inf horizon
> items remain, and none is a current obligation.

## M4 -- Coalescing by directory and file targets

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m4-coalescing-by-directory-and-file-targets).

## M5 -- Fault model and the retry protocol

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m5-fault-model-and-the-retry-protocol).

## M6 -- Coarse fallback

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m6-coarse-fallback).

## M7 -- Documentation, examples, stress

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m7-documentation-examples-stress).

## M8 -- Adopt wtf-string for relative names

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m8-adopt-wtf-string-for-relative-names).

## M9 -- Data-driven scenario stress

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m9-data-driven-scenario-stress).

## M9+ -- Concurrent modifiers, spoilers, nesting, and queue overwhelm

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-22----m9-concurrent-modifiers-spoilers-nesting-and-queue-overwhelm).

## M10 -- Failure detail on every fault report (D-79, PR #20 review response)

Fixes the review's core complaint directly: a client had no way to know *why* an open or arm failed, only
that it did (`FaultOperation::Open`/`Arm`), or, for a permanent stop, only the coarse `OpenFailure`
classification. Independent of M11/M12 below.

- [x] **M10.1** -- Add `FailureCode` (`Win32(u32)` / `HResult(i32)`, `#[non_exhaustive]`) and `FaultDetail`
  (`{ failure: OpenFailure, code: FailureCode }`) to `directory.rs`; add `OpenError::code() -> FailureCode`.

- [x] **M10.2** -- Change `WatcherInner::enter_fault` to take `(OpenFailure, FailureCode)` instead of a bare
  `io::Error`, storing both in `FaultState` (supersedes D-54). Fix every call site to classify at the
  source instead of re-wrapping through `io::Error::other` (the `retry_reestablish` open-class path
  currently does this, silently discarding its already-classified `OpenError`).

- [x] **M10.3** -- `Notification::RetryQuestion` and `Outcome::Failed` carry `detail: FaultDetail` instead
  of (nothing) / `failure: OpenFailure` respectively. Breaking change to already-published API
  (`Outcome::Failed`'s field), commit as `feat(file-watcher)!`.

- [x] **M10.4** -- Update the `log::warn!` diagnostics (D-58) to include the new detail, and every existing
  test that matches on `Outcome::Failed`/`RetryQuestion`.

- [x] **M10.5** -- Integration test: a permanent open failure (`NotADirectory`) reports its real
  `FailureCode` through `Outcome::Failed`; an interactive subscription's `RetryQuestion` for a retryable
  open failure reports a real `FailureCode` too. -> implemented with `InvalidPath` instead: `NotADirectory`
  turns out to be unreachable through `subscribe` in practice (a non-directory leaf is always retried as a
  file target, D-7, against its real parent, which succeeds) -- see
  [tests/fault_detail.rs](tests/fault_detail.rs).

## M11 -- Reopen identity: file-reference-based reopen, and volume-identity tracking (D-78 groundwork)

Closes two related bugs found while designing D-78: `WatcherInner::reopen` always re-resolves by path even
when its previous handle is still live, and `Resident.directories`'s `DirectoryId` key is never updated
after a reopen lands on a different directory. Independent of M10 above; M12 below depends on this.

- [x] **M11.1** -- Add `directory::VolumeIdentity` (filesystem name + volume label via
  `GetVolumeInformationByHandleW`, reusing the volume serial `DirectoryId` already computes) and a
  `DirectoryHandle` method wrapping `ReOpenFile`. -> `ReOpenFile` measured (D-52) to fail outright
  (`ERROR_ACCESS_DENIED`, needs `SeBackupPrivilege`); replaced with `DirectoryHandle::reopen_by_id`
  (`OpenFileById`, reopens by the file reference `DirectoryId` already carries) plus
  `DirectoryHandle::canonical_path` (`GetFinalPathNameByHandleW`, needed because `OpenFileById` is
  path-independent and would otherwise silently follow a moved/renamed directory). See D-80 and
  [Reopening by file reference, and why the fast path is gone](DESIGN-NOTES.md#reopening-by-file-reference).
  **Superseded by M15.2:** `reopen_by_id` is removed -- Windows rejects a directory-change read on a
  by-id open, so it could never produce a watchable handle.

- [x] **M11.2** -- `WatcherInner::reopen` tries `ReOpenFile` against its still-live previous handle first
  (the old endpoint is not torn down until after this succeeds or fails), falling back to the existing
  path-based `DirectoryHandle::open` only when that fails. Verify empirically (real-OS test, per this
  crate's D-52 precedent of measuring rather than assuming Win32 behavior) that `ReOpenFile` behaves as
  documented for a `FILE_FLAG_BACKUP_SEMANTICS` directory handle. -> `WatcherInner::reopen_via_existing_handle`
  implemented the `OpenFileById`-plus-`canonical_path` mechanism above but returned `None` unconditionally,
  pending root-cause of a failure then attributed to IOCP association. **Superseded by M15.2:** root-caused
  to an OS limitation with nothing to do with IOCP, and the whole fast path removed. Every reopen is
  path-based, which is what M11.3/M11.4 were already written against. See D-80.

- [x] **M11.3** -- Track each `DirectoryWatcher`'s current `VolumeIdentity`, recorded (no comparison) at
  first establish, compared only on the path-based fallback path -- a `ReOpenFile` success needs no
  comparison at all (D-78).

- [x] **M11.4** -- Fix the stale-`DirectoryId`-key bug: when the path-based fallback produces a
  `DirectoryId` different from the one `Resident.directories` currently keys this watcher under, re-key
  the map entry. -> `monitor::rekey`, called from `WatcherInner::on_path_based_reopen`.

- [x] **M11.5** -- Integration test: a manufactured reopen through `ReOpenFile` returns a handle to the same
  file (`DirectoryId` unchanged) while the original handle stays open; a deleted-and-recreated directory
  falls back to the path-based open and picks up its (possibly different) new identity, re-keying
  `Resident.directories` correctly. -> `monitor::tests`'s
  `a_path_based_reopen_that_lands_on_a_new_directory_rekeys_so_a_later_subscription_still_coalesces` covers
  the re-keying claim end to end. **Superseded by M15.2** for the file-reference half: the `reopen_by_id_*`
  identity tests are removed with the mechanism they characterised, and what replaces them is
  [tests/reopen_by_id_cannot_be_watched.rs](tests/reopen_by_id_cannot_be_watched.rs), which asserts the OS
  limitation that removal rests on.

## M12 -- Per-subscription volume-change confirmation (D-78)

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-23----m12-per-subscription-volume-change-confirmation-d-78).

## M13 -- Consumer test surface (`test-util`)

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-25----m13-consumer-test-surface-test-util).

## M14 -- Audit the delivery contract against the ten specification-gap categories (D-84)

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-27----m14-audit-the-delivery-contract-against-the-ten-specification-gap-categories-d-84).

## M15 -- Findings from the mutation-testing sweep

- [x] **M15.1** -- Resolved the unreachable half of `StandingHold::drop`: it was the drain path until `take` took the release over, and it could not have run safely -- reaching it deadlocks on the `items` lock its caller already holds. Replaced by an exercised tripwire. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m151)

- [x] **M15.2** -- Explained why a `reopen_by_id` handle rejects the watcher's own read: Windows refuses a directory-change read on any by-id open, holding access, mode, create options and resolved path identical. The fast path is removed, not disabled. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m152)

- [x] **M15.8** -- Settled the write-only tail of M15.2's removal: the stored `canonical_path` field is gone, `DirectoryHandle::canonical_path` stayed and now has a caller that uses its result plus the tests it never had. M15.3 stands, confirmed by injection. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m158)

- [x] **M15.3** -- Decided: a caller's path goes to Win32 verbatim, and long-path support is the consuming application's call, not this crate's (D-85). The proposed `\\?\` prefix was measured to break forward slashes, `.`, `..` and relative paths that work today. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m153)

- [x] **M15.9** -- Guarded D-85's pass-through with five identity-asserting tests. Measured worth: with a blanket prefix injected into `wide_path`, exactly those five fail and the other 33 in the module pass. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m159)

- [x] **M15.10** -- Tested `canonical_path`'s 512-unit retry. No junction needed: a caller's own `\\?\` path opens past `MAX_PATH` (D-85), so the retry is reachable through the crate's own API. Added a boundary walk over 508-516 units; `<=` proved equivalent by measurement. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m1510)

- [ ] **M15.4** -- Isolate the last two notification-filter categories, or record that they cannot be
  isolated from outside. Two mutants in `ALL_NOTIFY_FILTERS` survive: replacing the `|` before
  `FILE_NOTIFY_CHANGE_CREATION` (dropping LAST_WRITE and CREATION) and before
  `FILE_NOTIFY_CHANGE_SECURITY` (dropping CREATION and SECURITY). `&` binds tighter than `|`, so each
  such mutant zeroes the two flags on either side of it.
  **Why the obvious tests do not catch them, measured rather than assumed.** ATTRIBUTES and SIZE remain
  present in both mutants, and they mask the rest: a same-length rewrite still sets the file's archive
  bit, so ATTRIBUTES reports it; and a DACL edit via `icacls` is likewise still reported with SECURITY
  dropped, through some filter this exercise did not identify. Both tests were written expecting to
  isolate a category, both failed to, and both are kept with their claims corrected rather than deleted.
  **What would work.** For LAST_WRITE, a timestamp-only change -- `SetFileTime` on an already-open
  handle, touching neither length nor attributes. For SECURITY, first establish *which* filter currently
  reports a DACL edit (arm a watch with a single filter bit at a time and see which one fires), because
  the assumption that it touches nothing else is exactly what the failed test disproved.
  **A legitimate outcome is "cannot be isolated".** If every operation that changes one of these also
  changes an attribute or a length, then no black-box test can distinguish the mutants, and they belong
  with the equivalent ones rather than on this list. Establishing that is as good an answer as a test.

- [ ] **M15.5** -- Assert the arming contract in `arm_detailed_read`, so a broken one fails a test
  instead of *sometimes* corrupting the heap. **The shipping code is not at fault here** -- that was
  checked before anything else, and the check is recorded below so nobody has to repeat it.
  **What is wrong.** Inverting the `ERROR_IO_PENDING` test at `watcher.rs:482` makes a genuinely-pending
  read look failed, so the thread pool cancels its accounting for an I/O the kernel is still going to
  complete, and the completion lands in a freed buffer. Nothing asserts otherwise, so the only thing
  standing between that mutation and a green suite is whether the allocator happens to notice.
  **It is not reliable, and that is the point.** The same mutant was recorded `MISSED` in one sweep and
  crashed the process in another. Sixteen crashes were logged across the runs, and cargo-mutants counted
  two of them as `CaughtMutant` purely because the process exited non-zero -- one with
  `STATUS_HEAP_CORRUPTION` (`0xC0000374`), one with `STATUS_STACK_BUFFER_OVERRUN` (`0xC0000409`). **A
  crash is not a test.** Detection by memory corruption depends on allocator behaviour and heap layout,
  so the mutation score for this file is non-deterministic run to run, and a "caught" here is a weaker
  claim than it looks.
  **Wanted:** a test that observes the arming *contract* rather than its wreckage -- that a read
  reporting `ERROR_IO_PENDING` is treated as armed and its completion delivered exactly once, and that a
  genuinely failed submission is not left accounted-for. That makes both the mutant and any future
  regression a deterministic red test.
  **Ruled out: a defect in the unmutated code.** 55 runs of the unmutated suite (25 default-feature, 30
  `--all-features`, including the exact binary named in every crash report) produced zero failures. All
  sixteen crash reports carry distinct PE timestamps, none equal to the clean build's -- each was its own
  mutant build. The test binary's filename is derived from the target and features rather than its
  contents, which is why every report names the same `.exe` and why that name alone proves nothing.

- [x] **M15.6** -- Converted `queue/tests.rs` to bounded waiting, so a broken wake fails instead of hanging. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m156)

- [ ] **M15.7** -- Decide the test-side wait budget, so a mutation sweep is not dominated by tests that
  correctly fail slowly. **This is a throughput decision, not a test gap -- do not close it by writing tests.**
  **The measurement.** After M15.6, a full `queue.rs` sweep is 124 mutants in 20 minutes, and **14 x 67s =
  15.6 minutes of that is mutants scored `timeout`**. Every one of those 14 was already detected: between 4
  and 132 tests had `FAILED` before cargo-mutants killed the run. The kill happens because the suite exceeds
  3x the baseline, and it exceeds it because dozens of bounded waits each burn their full budget on the way
  to failing.
  **Where the budget lives.** `NOTIFY_TIMEOUT` in [src/watcher/tests.rs](src/watcher/tests.rs) is
  `Duration::from_secs(30)`, plus several 5s and one 20s bound. Those numbers are generous on purpose --
  they are what keeps the suite from flaking on a loaded machine -- so lowering them trades sweep throughput
  against exactly that robustness. That trade is the decision, and it is the engineer's.
  **The options, none free.** (a) Lower `NOTIFY_TIMEOUT` and accept more flake risk under load. (b) Raise
  `--timeout-multiplier` in [tools/run-mutants.ps1](../../tools/run-mutants.ps1) so a suite full of slow
  failures still fits, which makes a genuine wedge cost proportionally more. (c) Leave it, and read
  `timeout` as "detected" rather than "unknown" -- correct today, but only because it was checked by hand,
  and nothing keeps it true.
  **Read `missed` as the gap column.** After M15.6, `timeout` no longer distinguishes a wedge from a slow
  detection, so a sweep's `timeout` list has to be adjudicated by counting `FAILED` lines in each log before
  it means anything.

## M-inf -- Horizon (ungated, post-v1)

Parked, not pending. These are the deferred seams recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> D-19,
an explicit design decision that places them outside the v1 scope. That recorded decision -- not the
absence of a current consumer -- is why each is deferred, which is a legitimate deferral rationale (a
resolved, recorded scope decision), not a scope-boundary excuse. Each graduates to a numbered milestone
when a post-v1 line of work takes one up. None is an open obligation of any current milestone.

- [ ] **M-inf.1** -- `ReadDirectoryChangesExW` extended records (`FILE_NOTIFY_EXTENDED_INFORMATION`): surface the
  richer per-record fields on OS versions that support it, behind capability detection, without disturbing
  the basic `FILE_NOTIFY_INFORMATION` surface (D-18/D-19). **Availability is per-filesystem, not merely
  per-OS-version:** even on a build that exposes the API, some filesystems reject the extended structure --
  e.g. ReFS still does not support it (for no good reason) -- so detection must probe the actual target
  volume and fall back to `FILE_NOTIFY_INFORMATION`, never inferring support from the OS version alone.

- [ ] **M-inf.2** -- Digest-based change *verification*: an optional mode that confirms a reported change by
  hashing content, trading cost for fewer spurious notifications (D-19).

- [ ] **M-inf.3** -- Per-volume capability cache: remember detailed-vs-coarse (and extended-record) support per
  volume so establish/re-establish need not re-probe each time (D-17/D-19).

- [x] **M-inf.4** -- Root-caused M11.2's fast reopen path: not an IOCP defect at all, but Windows refusing a directory-change read on any by-id open, so the path was removed rather than fixed. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m152)
