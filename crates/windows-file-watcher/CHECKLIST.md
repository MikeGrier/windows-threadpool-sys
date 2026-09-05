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

> **NEXT ACTIONABLE ITEM: none.** M1 through M16 are done. Only the parked, ungated M-inf horizon items
> remain, and none is a current obligation.

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

- [x] **M15.4** -- Isolated both remaining notification-filter categories. All six `ALL_NOTIFY_FILTERS` flag-pair mutants are now caught. Two of the item's own recorded claims were disproved by measurement: ATTRIBUTES does not mask a same-length rewrite, and a DACL edit *is* reported by SECURITY alone. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m154)

- [x] **M15.5** -- Made the arming contract observable: extracted `classify_submission` (taking the raw `BOOL`, so the `!= 0` convention is inside the tested surface too) and asserted all four cases. Every mutant now fails as a deterministic red test rather than as a heap corruption. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m155)

- [x] **M15.6** -- Converted `queue/tests.rs` to bounded waiting, so a broken wake fails instead of hanging. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m156)

- [x] **M15.7** -- Decided and implemented: `NOTIFY_TIMEOUT` lowered 30s -> 5s across all three copies, after measuring that 45 of 46 waits finish in <=2.5ms and the whole tail is one structural ~515ms backoff, unchanged under 4x oversubscription. One previously-timing-out mutant: 93.6s -> 31.8s. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m157)

- [x] **M15.11** -- Bounded the loop in `no_wakeup_is_lost_under_a_concurrent_burst` (a bounded wait inside an unbounded loop is still an unbounded loop) and aligned `await_signal`'s budget with M15.7's. The suite-wide sweep for the same shape found no other instance. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m1511)

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
