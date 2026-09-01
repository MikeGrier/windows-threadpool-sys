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

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-27----m14-audit-the-delivery-contract-against-the-ten-specification-gap-categories-d-84).

## M15 -- Resolve the unreachable half of `StandingHold::drop`

- [ ] **M15.1** -- Decide whether `StandingHold::drop`'s release path is dead code, and act on the
  answer. **Found by mutation testing, and it is a reachability question rather than a test gap** --
  which is why it is queued for a decision instead of being closed with a test.
  Three mutants in that `Drop` survive: `state.reserved += 1` changed to `-=` and to `*=`, and the `!`
  deleted from `if !standing.slot_alive`. Each was re-injected on its own line and confirmed to leave the
  suite green, so this is not an artifact of the run's feature flags.
  **No test can catch them as the code stands.** The only pop from `state.queue` is in `take` (one call
  site), and `take` performs the release inline and sets `resolved = true`, so `Drop` returns at its first
  line for every drained entry. An *undrained* entry's hold is only dropped when `Shared` itself is torn
  down -- and then `self.shared.upgrade()` returns `None` and `Drop` returns at its second line. Nothing
  reaches the body in between.
  The doc comment says `Drop` "remains the fallback for every other discard", so either a discard path was
  intended and never built, or one existed and was removed when `take` took over the release (a PR #20
  review response, per the comment beside it). Both readings are plausible from the code alone; the
  engineer who made that change can tell them apart, and an assistant deleting live-looking accounting on
  a hunch is exactly the wrong move.
  Three outcomes are legitimate: **remove** the unreachable body if the discard path is genuinely gone;
  **keep it and say why** if it guards a path that is coming (recording that here, so the next mutation
  run does not re-litigate it); or **build the missing path** if its absence is itself the defect. What is
  not legitimate is adding a test that reaches it artificially -- that would manufacture coverage for code
  nothing calls.

- [ ] **M15.2** -- Explain, then either fix or document, why a handle from `reopen_by_id` **rejects the
  very read the watcher exists to issue**. Found while chasing a surviving mutant; the mutant is the
  symptom and this is the disease.
  **The measurement.** Open a temp directory with `DirectoryHandle::open`, reopen it with
  `DirectoryHandle::reopen_by_id`, and issue the same overlapped `ReadDirectoryChangesW` on each
  (DWORD-aligned buffer, `FILE_NOTIFY_CHANGE_FILE_NAME`, null `lpBytesReturned`, an `OVERLAPPED`, no
  completion routine). The **original** handle accepts it -- returning TRUE with the operation pending,
  which the call site in `watcher.rs` documents as normal. The **reopened** handle fails it with
  `ERROR_INVALID_PARAMETER` (87).
  **Why it matters.** `reopen_by_id` requests `FILE_LIST_DIRECTORY` and
  `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED` through `OpenFileById`, which reads as a handle
  fit for watching. If it is not, the reopen path either cannot serve a watch at all, or serves it only
  because some later step re-derives a usable handle -- and which of those is true is not visible from
  the code.
  **Three readings, and the engineer can tell them apart faster than a probe can.** (a) A real defect in
  the reopen path. (b) A legitimate difference in what `OpenFileById` returns (an access right such as
  `SYNCHRONIZE`, or volume-hint semantics) that the reopen path compensates for elsewhere. (c) A defect
  in the measurement above, though it was run as a control against the original handle in the same
  process and the original passed.
  **What this explains.** `directory.rs:457`'s `|` -> `&` mutant survives -- the one that zeroes both
  flags -- because every `reopen_by_id` test asserts only *which* directory came back, never that the
  handle is usable afterwards. No test can close that gap until the behaviour above is understood, so
  writing one now would encode whichever answer happened to be true.
  A test asserting handle usability was written and then **removed rather than committed red**; it is
  reconstructible from the measurement recorded here.

- [ ] **M15.3** -- Decide whether this crate should open paths longer than `MAX_PATH`, and note the
  consequence for `canonical_path`'s retry either way.
  **`wide_path` passes the caller's path to `CreateFileW` verbatim, with no `\\?\` prefix**, so a
  directory deeper than `MAX_PATH` fails to open with `ERROR_PATH_NOT_FOUND` even though
  `std::fs::create_dir_all` will happily create it (Rust prefixes internally). Measured while building a
  fixture for the item below: the directory existed on disk and `DirectoryHandle::open` refused it.
  **The knock-on.** `canonical_path` sizes a 512-unit buffer and retries when
  `GetFinalPathNameByHandleW` says the path did not fit. Reaching that retry needs a canonical path of
  512+ units, and the only way in through `open` is a path longer than `MAX_PATH` -- which cannot be
  opened. So the retry is unreachable via the crate's own API, which is why its `<` survives being
  changed to `>` and `<=` (the `>` case loops forever and shows up as a timeout rather than a failure).
  There *is* a back door -- a short junction pointing at a deep target, since
  `GetFinalPathNameByHandleW` returns the resolved target -- so the code is not dead, merely unreachable
  by the obvious route. That is the fixture to build if the retry is worth testing as it stands.
  **Two coherent outcomes.** Support long paths (prefix `\\?\` in `wide_path`, which also makes the
  retry reachable and testable), or state the `MAX_PATH` limit as deliberate and note that the retry
  covers only the junction case. Silence is the one option that leaves a caller to discover the limit
  from a `NotFound` that names nothing.

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
