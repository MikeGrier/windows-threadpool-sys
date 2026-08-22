# Completed checklist: windows-file-watcher

Append-only archive of completed milestones moved out of [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-08-19 -- M1: crate scaffold and notification decode

- [x] **M1.1** -- Scaffold `crates/windows-file-watcher`: [Cargo.toml](Cargo.toml) with a `cfg(windows)`-gated `lib`,
  path+version dependencies on [windows-overlapped-io-sys](../windows-overlapped-io-sys/README.md) and
  [windows-threadpool-sys](../windows-threadpool-sys/README.md), and `windows-sys` with the needed feature
  groups; [src/lib.rs](src/lib.rs) crate-doc skeleton; add the crate to the workspace members. Everything is
  `cfg(windows)`, so the crate resolves to an empty crate elsewhere.

- [x] **M1.2** -- Seed Tier-1 [DESIGN-NOTES.md](DESIGN-NOTES.md) and Tier-2 [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md)
  from [the design session](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md) (D-1...D-20), and
  wire the crate into CI so the default and `--all-features` configurations both build, test, and lint.

- [x] **M1.3** -- `FILE_NOTIFY_INFORMATION` record-walk decoder: follow the `NextEntryOffset` chain and
  extract `Action` plus the UTF-16 `FileName` (`FileNameLength` bytes, not NUL-terminated) into a lossless
  relative-name type exposing both `OsString`/`Path` (WTF-8) and raw `&[u16]` (D-8). Malformed offsets are
  handled without out-of-bounds reads.

- [x] **M1.4** -- Change-record surface: map raw `FILE_ACTION_*` to a `ChangeKind` that keeps
  `RenamedOldName` / `RenamedNewName` distinct (D-9); a batch type; and recognition of the zero-byte
  completion as overflow -> `Desync { Overflow }` at the decode boundary (D-12).

- [x] **M1.5** -- Tests: >=10 normal decode cases plus edge cases (empty buffer, single record, multi-record
  chains, long names past `MAX_PATH` and a name filling the record to the buffer edge -- the crate imposes no
  maximum, `FileNameLength` being a `u32` bounded only by the completion buffer -- empty and unpaired-surrogate
  names, and malformed buffers: truncated/overrunning/odd-length and unaligned or out-of-range
  `NextEntryOffset`, all surfacing as `Desync`). Integration: decode a buffer produced by a real overlapped
  `ReadDirectoryChangesW` on a temp-directory mutation.

**Later addition -- D-21.** Review after this group was archived found the decoder accepted a trailing
remainder it should not have: it bounded the tail rather than requiring an exact length. Because a name is a
whole number of UTF-16 units, a record always ends on an even offset, so its DWORD alignment padding is
exactly 0 or 2 bytes -- never 1 or 3. Anything else is undescribed data and is now reported as a desync
rather than silently dropped, which would understate the batch. Fixed in M1.3/M1.5 above (a failing
regression test was written first) and recorded as `D-21` in [DESIGN-NOTES.md](DESIGN-NOTES.md); the
rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Noted here so the archive reflects that this
milestone's decoder gained a binding decision after it closed.

## Moved 2026-08-21 -- M2: the detailed single-directory watcher

The arm / complete / re-arm loop against `ReadDirectoryChangesW`, from opening the directory handle to
tearing it down. Added decisions D-22 (open-failure classification), D-23 (the arm gate is a lock held
across the submission), D-24 (the completion buffer is a heap-indirected `Box<[u32]>`), D-26 (an empty
completion is not a notification), D-34 (the gate names why it is closed; teardown is one idempotent
operation) and D-35 (how the loop is tested, and why its overflow is forced rather than raced).

- [x] **M2.1** -- Owned directory handle: `CreateFileW(FILE_LIST_DIRECTORY, FILE_SHARE_READ|WRITE|DELETE,
  OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)`; classify open errors (retryable vs
  not-found vs unsupported).

- [x] **M2.2** -- Arm and complete: issue `ReadDirectoryChangesW` through `windows-threadpool-sys`
  `ThreadpoolIo` (the overlapped seam with the generation-stamped identity, D-3/D-4); decode the completion
  into a batch (M1) and re-arm around processing to minimise the inherent loss window.

- [x] **M2.3** -- Deliver batches into a crate-owned queue endpoint (the interim, entirely in-crate delivery
  target for this milestone; the session/receiver split lands in M3, D-11) so the crate never calls into
  client code on its cadence path; tag records with a `WatchId`; emit `Desync { Overflow }` on a zero-byte
  completion.

- [x] **M2.4** -- Teardown: cancel the outstanding read, drain the pool I/O, and free the context via
  owned-object `Drop` (D-20), with re-arm suppression inherited from `ThreadpoolIo` rundown. The arm gate of
  D-23 already provides the suppression; formalise it, and note that the same not-re-arming state is what
  D-29 reuses for backpressure and D-28 for faults.

- [x] **M2.5** -- Integration: create/modify/delete/rename in a temp directory and assert raw actions and
  relative names; force a burst overflow and assert `Desync { Overflow }`; assert clean teardown with an
  operation outstanding.

**Carried forward.** M2 left one deliberate piece of scaffolding: the `unstable-internals` feature and the
`crate::unstable` module of D-35, which exist only so this milestone's integration test can reach a
`pub(crate)` loop. Its removal is queued as part of **M3.8** in [CHECKLIST.md](CHECKLIST.md), together with
the three `#![allow(dead_code)]` suppressions it stands in for. It is noted here so the archive does not
read as if M2 closed with nothing outstanding.

## Moved 2026-08-21 -- M3: monitor, session, watch handle, and the two queues

The queue-mediated model, end to end: a `Monitor` servicing requests on one serialised path, `Session`s
binding a submission handle to a notification destination, an affine `Watch`, a bounded notification queue
with the reservation discipline, and a doorbell on each direction. M3.8 also retired the `unstable-internals`
scaffolding of D-35, so the crate now ships its real public surface.

Added decisions D-36/D-37 (the SQ doorbell's edge is "a drain is outstanding", and `Request` was uninhabited
until it had variants), D-38 (a session is a handle on the monitor, not a co-owner), D-39/D-40 (the latch is
drained into the queue at the next enqueue; the bound is a `NonZeroUsize`), D-41/D-42 (the doorbell is one
predicate re-established under the queue lock, handed out borrowed and owned), D-43/D-44 (identifiers are
per-monitor and registration options are non-exhaustive; registration's failure is not the caller's return
value), D-45/D-46 (a subscription reserves two slots because `Drop` cannot report a refusal; a retryable open
is `Establishing`), and D-47/D-48 (resuming needs a re-check under the gate lock, and happens on this crate's
own pool).

Two items were corrected during execution rather than after: M3.1's doorbell condition (which coalesced but
did not serialise) and M3.2's bound parameter (which had no meaning until M3.3 defined one, and moved there).

- [x] **M3.1** -- `Monitor`: owns the servicing path; the request queue is drained by a `ThreadpoolWork`
  that serialises resident-state mutations (D-2); `Monitor::Drop` blocks on full rundown (D-20). Includes
  the SQ doorbell (D-25), ringing only when no drain is already outstanding.

- [x] **M3.2** -- `Session` obtained from the monitor: bundles a request-submission handle (MPSC producers)
  and the crate-owned notification sender (D-2/D-11); `monitor.session()` returns the session plus the
  client-side receiver.

- [x] **M3.3** -- Bound the notification queue (D-11) and add the reservation discipline (D-33): control
  reserves its capacity before proceeding, observation reserves nothing and stays best-effort, and the
  per-`WatchId` coalescing latch reports a loss that could not be enqueued.

- [x] **M3.4** -- The CQ doorbell (D-25): a lazily created manual-reset event, so a client can drain from
  its own `ThreadpoolWait` rather than dedicating a thread to a blocking `recv()`.

- [x] **M3.5** -- Affine `Watch` (D-5): `#[must_use]`, `Drop` enqueues cancellation, explicit `cancel()`,
  and a `Copy` `WatchId`; subscribe/cancel plumbed through the serialised request queue, with the D-27
  retry mode stated at registration.

- [x] **M3.6** -- Request completions (D-30), reserved at submit (D-33), including the permanent subscribe
  failures of D-22.

- [x] **M3.7** -- Backpressure (D-29): control needs no throttle, and observation is throttled at the arm
  so saturation becomes a grace period in the kernel's change buffer rather than a loss.

- [x] **M3.8** -- Integration through the public surface, and retirement of the `unstable-internals`
  feature, the `unstable` module, and the three `#![allow(dead_code)]` suppressions it stood in for.


## Moved 2026-08-21 -- M4: coalescing by directory and file targets

- [x] **M4.1** -- Coalesce watchers by directory (D-6): union the `FILE_NOTIFY_CHANGE_*` filters and take the
  maximum subtree flag across a directory's subscriptions; issue one read per directory. Directory identity
  is by file (volume serial + file index, D-50), not path string, so different spellings of the same
  directory still coalesce.

- [x] **M4.2** -- De-multiplex on decode: route each record to the subset of subscriptions whose target and
  filter match (per-subscription filtering, D-6). Implemented as `Route::select` filtering each decoded batch
  per route; every coalesced watcher publishes once per completion but sends a distinct per-route
  notification.

- [x] **M4.3** -- File (path) targets (D-7): watch the parent directory non-recursively and filter the leaf
  name; directory targets optionally recursive. A path traversing through a file (e.g. `file/nested`)
  surfaces as `NotFound`/retryable on Windows (`ERROR_PATH_NOT_FOUND`), not a permanent `NotADirectory`
  failure -- a distinction discovered empirically and recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] **M4.4** -- Add/remove a subscription to/from an existing coalesced directory watcher without
  disturbing the others' cadence. **Corrected during execution (D-52):** the item's original wording assumed
  a cancel-and-resubmit re-issue on the same handle; measurement showed the kernel does not honor a widened
  `bWatchSubtree` on a resubmitted read on the same handle -- only direct children kept being reported.
  Widening instead reopens the directory (fresh handle, fresh `ThreadpoolIo`), a real scope increase raised to
  and approved by the engineer per the PRIME DIRECTIVE before implementing.

- [x] **M4.5** -- Integration: several file-watches plus a recursive directory watch within one tree; assert
  each subscription receives exactly its matching events and nothing else.
  ([tests/watched_paths.rs](tests/watched_paths.rs))


## Moved 2026-08-21 -- M5: fault model and the retry protocol

- [x] **M5.1** -- Establish/re-establish state machine (D-14/D-15). **Corrected during execution (D-53):**
  rather than a separately named `Opening -> ArmingDetailed -> WatchingDetailed` machine, the existing
  `ArmGate` (already used for backpressure and widening) gained a fourth variant, `Faulted` -- it already
  answered exactly the question a fault needs answered. A `Pending` subscription's own open-retry (before any
  directory identity exists) and a coalesced watcher's arm-retry are two independent instances of the same
  protocol (D-59), each with its own `ThreadpoolTimer`. Classification: every arm-completion error is
  arm-and-retry (D-15); reopening (M4.4's existing mechanism) classifies its own open attempt through
  `OpenError::failure()` -- retryable re-enters the fault loop, permanent is the one edge that still ends in
  the pre-existing terminal `stopped` state. No downgrade-to-coarse edge yet (that is M6).

- [x] **M5.2** -- The fault latch. **Corrected during execution (D-54/D-55):** the interactive fault
  question is delivered through a standing per-subscription reservation (`StandingSlot`, taken once at
  registration) rather than the resident coalescing latch originally sketched -- sound because a watcher
  cannot fault twice concurrently, so one permanent slot is always enough. `FaultState` retains only which
  operation faulted, not the triggering error (surfaced once via the M5.6 diagnostic and then discarded).

- [x] **M5.3** -- The retry protocol (D-27, superseding D-16): defaults vs. interactive at registration,
  earliest-answer-wins with a decliner counted at the default, clamped to the floor (500 ms default,
  50 ms floor, separate for open vs. arm). **Scope note (D-56):** fixed per-operation defaults only -- no
  growth multiplier, cap, jitter, or per-error-kind override, since `RetryMode`/`WatchOptions` carry no field
  for any of them.

- [x] **M5.4** -- Recovery notifications: `Desync { Reestablished }` (unconditional, like every other
  desync) plus the opt-in `Suspended`/`Resumed`/`Established { mode }` brackets (D-13). **Corrected during
  execution (D-57):** these liveness brackets ride the ordinary best-effort observation queue, like `Desync`
  -- only `RetryQuestion` itself needs D-55's standing reservation.

- [x] **M5.5** -- Cancellation from any intermediate state: removing the last route a fault is still
  awaiting an answer from resolves that fault immediately (counted as a decline) rather than leaving it
  wedged; a `Pending` subscription's own retry timer and standing slot are dropped the same way on cancel.

- [x] **M5.6** -- Observable stall and diagnostics (D-31/D-58): the `log` facade at two call sites (entering
  a fault, resolving one); `DirectoryWatcher::is_faulted` and `Monitor::is_faulted` expose the state directly.

- [x] **M5.7** -- Integration (D-59): delete then recreate the watched directory. Confirmed empirically that
  deleting a directory with an outstanding `ReadDirectoryChangesW` does fail that read (an arm-class fault),
  and that recreating it lets the reopen loop re-establish, reporting `Suspended` -> `Resumed` /
  `Established` / `Desync { Reestablished }`, with real subsequent changes reported again afterward.
  ([tests/watched_paths.rs](tests/watched_paths.rs))


## Moved 2026-08-21 -- M6: coarse fallback

- [x] **M6.1** -- Coarse handle: `FindFirstChangeNotificationW`/`FindNextChangeNotification`/
  `FindCloseChangeNotification`, owned and closed with the custom-close routine (not `CloseHandle`),
  reaching `ThreadpoolWait` through `WaitableHandle::assume_waitable_with`. ([src/coarse.rs](src/coarse.rs))

- [x] **M6.2** -- Coarse watcher: `ThreadpoolWait` per activation -> `FindNextChangeNotification` (before
  re-arming, since the handle stays signalled until that is called) -> emit `Desync { Coarse }`, under the
  same backpressure/fault discipline as detailed reads (D-15/D-17/D-29): a directory's single
  `WatcherInner` now dispatches its arm/teardown/backpressure logic on whichever tier (`Endpoint::Detailed`
  or `Endpoint::Coarse`) is currently installed, rather than duplicating that machinery for a second type.

- [x] **M6.3** -- Downgrade edge in `reopen` (D-17): detailed is attempted first; only a failure that
  `directory::classify` reports as `OpenFailure::Unsupported` falls back to opening coarse over the same
  path, and every other failure propagates as an ordinary rearm-and-retry fault (D-15). Mode is re-resolved
  on every `reopen` call, so this also covers a re-establish attempt after a fault, and the very first
  establishment (the constructor now calls `reopen`, not a hand-built detailed-only path, unifying all three
  cases behind one mechanism).

- [x] **M6.4** -- `Established { mode }` (D-13, opt-in) now reports whichever tier actually settled
  (`WatcherInner::mode`), not a hardcoded `Detailed`, and fires on every successful establishment including
  the very first (a fix to an M5-era gap: the notification's own contract was "reported once at first
  establishment and again after every re-establishment", but the M5 code only sent it on a later success).
  The test seam is `DirectoryWatcher::start_forcing_coarse` (`#[cfg(test)]`), which skips the detailed
  attempt unconditionally via a `force_coarse: AtomicBool` field.

- [x] **M6.5** -- Integration: force coarse via the seam, assert a real file change surfaces as
  `Desync { Coarse }`, assert a recovered fault in forced-coarse mode reports `Established { Coarse }`
  alongside `Resumed`/`Desync { Reestablished }`, and assert coarse teardown converges promptly.
  **Corrected during execution:** the seam (`start_forcing_coarse`) is `pub(crate)`, so this lives in
  [src/watcher/tests.rs](src/watcher/tests.rs) (a crate-internal unit test with real-OS behaviour) rather
  than the external `tests/` integration crate, which can only reach `pub` items -- matching M3.8's
  retirement of the `unstable-internals` feature-gated-public-seam pattern rather than reintroducing it.
