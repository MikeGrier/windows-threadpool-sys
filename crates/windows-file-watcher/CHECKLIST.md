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

> **NEXT ACTIONABLE ITEM: none.** M1 through M13 are archived/done. Only the parked, ungated M-inf horizon
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
  [Reopening by file reference, and why the fast path is off](DESIGN-NOTES.md#reopening-by-file-reference-and-why-the-fast-path-is-off).

- [x] **M11.2** -- `WatcherInner::reopen` tries `ReOpenFile` against its still-live previous handle first
  (the old endpoint is not torn down until after this succeeds or fails), falling back to the existing
  path-based `DirectoryHandle::open` only when that fails. Verify empirically (real-OS test, per this
  crate's D-52 precedent of measuring rather than assuming Win32 behavior) that `ReOpenFile` behaves as
  documented for a `FILE_FLAG_BACKUP_SEMANTICS` directory handle. -> `WatcherInner::reopen_via_existing_handle`
  implements the `OpenFileById`-plus-`canonical_path` mechanism above, but returns `None` unconditionally:
  measured to hang or (once) crash the process with `STATUS_STACK_BUFFER_OVERRUN` once a handle obtained
  this way is associated with the thread pool's IOCP and armed, for a reason not yet root-caused. Every
  reopen therefore uses the path-based fallback only, which is fully implemented and tested (M11.3/M11.4
  below do not depend on the fast path). See D-80.

- [x] **M11.3** -- Track each `DirectoryWatcher`'s current `VolumeIdentity`, recorded (no comparison) at
  first establish, compared only on the path-based fallback path -- a `ReOpenFile` success needs no
  comparison at all (D-78).

- [x] **M11.4** -- Fix the stale-`DirectoryId`-key bug: when the path-based fallback produces a
  `DirectoryId` different from the one `Resident.directories` currently keys this watcher under, re-key
  the map entry. -> `monitor::rekey`, called from `WatcherInner::on_path_based_reopen`.

- [x] **M11.5** -- Integration test: a manufactured reopen through `ReOpenFile` returns a handle to the same
  file (`DirectoryId` unchanged) while the original handle stays open; a deleted-and-recreated directory
  falls back to the path-based open and picks up its (possibly different) new identity, re-keying
  `Resident.directories` correctly. -> `directory::tests` covers the file-reference-reopen identity claims
  (`reopen_by_id_*`, including the rename hazard the fast path's disablement is about); `monitor::tests`'s
  `a_path_based_reopen_that_lands_on_a_new_directory_rekeys_so_a_later_subscription_still_coalesces` covers
  the re-keying claim end to end.

## M12 -- Per-subscription volume-change confirmation (D-78)

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-23----m12-per-subscription-volume-change-confirmation-d-78).

## M13 -- Consumer test surface (`test-util`)

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-25----m13-consumer-test-surface-test-util).

## M14 -- Audit the delivery contract against the ten specification-gap categories (D-84)

PR #42 found gaps in this crate's contract prose one at a time, reactively, over 19 review rounds. Each was
fixed where it was found, and [D-84](DESIGN-NOTES.md) records which decisions were amended. What has **not**
been done is the converse: a deliberate pass over the contract asking, for each of
[the ten categories](../../DESIGN-NOTES.md#specifying-a-delivery-contract), whether this crate states it or
leaves it to omission. Reactive fixes cannot establish that, because they only ever reach the categories some
reviewer happened to probe.

- [x] **M14.1** -- Audit [D-12](DESIGN-NOTES.md) (`Desync`), [D-27](DESIGN-NOTES.md)/[D-28](DESIGN-NOTES.md)
  (the fault protocol), and [D-30](DESIGN-NOTES.md) (request completions) against all ten categories, and
  state each answer -- including "unspecified, deliberately" where that is the honest one. These three carry
  the sequencing rules a consumer builds recovery on, so they are the highest-value targets. Done: see
  [The M14 audit](DESIGN-NOTES.md#the-m14-audit). Found three shipped documentation defects (`DesyncCause`'s
  type-level doc contradicting its own `Stopped` variant, a four-of-five cause enumeration, and a
  `Delivery and saturation` section still describing the pre-D-29 drop policy and the phrasing D-39 corrects)
  and two load-bearing rules stated nowhere (the standing slot's mutual-exclusion invariant, and that
  "every request produces a completion" holds for lifecycle requests only).

- [x] **M14.2** -- Audit the remaining notification-shaping decisions ([D-10](DESIGN-NOTES.md),
  [D-13](DESIGN-NOTES.md), [D-17](DESIGN-NOTES.md), [D-26](DESIGN-NOTES.md), [D-57](DESIGN-NOTES.md)) the
  same way, and fold any newly-stated rule into the harness's `schedule` module docs so the generator and
  the contract stay in step. Done: see [The M14 audit](DESIGN-NOTES.md#the-m14-audit). Three sequences the
  contract did not admit -- a liveness bracket can open with `Resumed` (a route coalescing onto an
  already-faulted watcher) and can close with `Desync { Stopped }` instead of `Resumed`; `Established` is
  not necessarily a watch's first notification; and the tier is re-resolved on every reopen, so it may
  differ between establishments. All three folded into the harness `schedule` docs.

- [x] **M14.3** -- Sweep for the [`has_room`](DESIGN-NOTES.md#the-has_room-finding-in-this-crate) shape:
  every advisory predicate this crate exposes or consumes on a reliability path, checked for whether its
  stated contract holds under the condition its caller actually uses it in, with a test in that condition.
  `has_room` was found by review rather than by looking; nothing establishes it was the only one. Done: see
  [The M14.3 predicate sweep](DESIGN-NOTES.md#the-m143-predicate-sweep). Nine predicates swept, one defect
  found -- `Receiver::is_empty` excludes owed latched losses and the end of the stream, so a client that
  waits on the doorbell (signalled on all three, D-41) and then tests it spins without ever collecting the
  loss report. `Receiver::has_pending` now publishes D-41's own predicate; three regression tests added.

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

- [ ] **M-inf.4** -- Root-cause and, if fixed, re-enable M11.2's fast reopen path
  (`WatcherInner::reopen_via_existing_handle`, currently hard-coded to return `None`): a handle obtained via
  `OpenFileById` hangs, or once crashed the process with `STATUS_STACK_BUFFER_OVERRUN`, once associated
  with the thread pool's IOCP and armed (D-80). `DirectoryHandle::reopen_by_id`/`canonical_path` are each
  independently correct per `directory::tests`; the defect is specifically in the IOCP-association/arm
  path against such a handle. Deferred because it needs dedicated low-level debugging (likely a minimal
  repro outside this crate) rather than blocking M11/M12 on it -- the path-based-only reopen it falls back
  to is fully correct, just without the optimization.
