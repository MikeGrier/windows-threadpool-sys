# Completed checklist: windows-file-watcher

Append-only archive of completed milestones moved out of [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-08-23 -- M12: per-subscription volume-change confirmation (D-78)

- [x] **M12.1** -- `VolumeChangeDecision` (`Continue`/`Stop`) and `VolumeChangePolicy` (`AutoContinue`
  default, `Confirm`) types; `WatchOptions::on_volume_change` setter. -> `directory::VolumeIdentity` made
  `pub` (with lossy `filesystem_name()`/`volume_label()` accessors) so it can be carried on
  `Notification::VolumeChanged` (M12.3).

- [x] **M12.2** -- New `ArmGate::VolumeChangePending` variant, entered when a path-based fallback reopen's
  `VolumeIdentity` differs from a route's stored baseline and that route opted into `Confirm`; resolves
  back to `Open` once every asked route on that directory has answered (D-47-style re-check under the gate
  lock). -> `WatcherInner::on_path_based_reopen` detects the change and defers installing the already-open
  candidate handle (held in the new `VolumeChangeState`/`self.volume_change`) until
  `WatcherInner::resolve_volume_change` runs.

- [x] **M12.3** -- `Notification::VolumeChanged { watch, previous, current }` and
  `Session::answer_volume_change(watch, VolumeChangeDecision)`, mirroring `RetryQuestion`/`Session::answer`.
  -> Reuses the same standing `fault_slot` reservation `RetryQuestion` does (widened at registration to
  cover `retry == Interactive || on_volume_change == Confirm`); the two questions are never outstanding at
  once for one subscription.

- [x] **M12.4** -- Wire the per-route resolution: `Stop` removes just that subscription via the existing
  `remove_route` path; `Continue` updates that route's own baseline `VolumeIdentity` to the one just
  confirmed; `AutoContinue` routes are never asked and are unaffected either way; the directory tears down
  normally through the pre-existing zero-routes path if this leaves none. -> `Continue`'s baseline update
  is the shared `WatcherInner::volume_identity` field (M11.3), advanced by `install()` itself once the
  deferred candidate handle is finally installed -- not duplicated per route, since every route on one
  coalesced directory necessarily shares one arm/volume history (D-6).

- [x] **M12.5** -- Cancellation-mid-question handling, mirroring D-27/M5.5's "leaving counts as declining"
  rule: a route removed while its volume-change question is outstanding resolves as `Stop` for that route,
  rather than wedging the awaiting set forever. -> `WatcherInner::remove_route_from_volume_change`, called
  from `DirectoryWatcher::remove_route`.

- [x] **M12.6** -- Integration test: two subscriptions on one coalesced directory, one `Confirm` one
  `AutoContinue`; force a `VolumeIdentity` change (a real removable-media swap where available, or a new
  test seam -- M11.2's fast reopen path is disabled, so there is no `ReOpenFile`-based seam to reuse here)
  and assert the `Confirm` route receives `VolumeChanged`, declines, and is removed, while the
  `AutoContinue` route keeps receiving batches uninterrupted. -> Implemented as two `watcher::tests` (a
  real end-to-end `Monitor`/`Session` integration test cannot force a volume change without real removable
  media): `VolumeIdentity::synthetic` (`#[cfg(test)]`) rigs a baseline guaranteed to differ from the real
  directory's actual volume, so a genuine `WatcherInner::retry_reestablish()` drives the real detection,
  question, and resolution paths. `only_the_confirm_route_is_asked_and_continuing_keeps_both_routes` and
  `stopping_a_volume_change_removes_only_that_route` cover `Continue` and `Stop` respectively, the latter
  also confirming the surviving `AutoContinue` route keeps receiving real batches afterward.

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


## Moved 2026-08-21 -- M7: documentation, examples, stress

- [x] **M7.1** -- Crate [README.md](README.md) plus expanded [lib.rs](src/lib.rs) top-level docs: the
  monitor/session/watch model, the two queues and their doorbells (D-25), the fidelity-and-limitation
  contract, the `Desync` primitive, and the D-27 retry protocol (defaults vs. interactive, the 500ms
  default / 50ms floor).

- [x] **M7.2** -- Runnable examples: [minimal_directory_watch.rs](examples/minimal_directory_watch.rs),
  [single_file_watch.rs](examples/single_file_watch.rs), and
  [fault_recovery.rs](examples/fault_recovery.rs) (interactive retry, answering every `RetryQuestion`).

- [x] **M7.3** -- Added D-53...D-65's missing Tier-2 rationale to
  [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md): the arm-gating deadlock (D-23), why open failures split
  permanent/retryable (D-22), the fault latch's evolution from resident state to a standing reservation
  (D-28/D-55), why the retry protocol ships fixed delays rather than a policy-reduction engine (D-27/D-56),
  why the monitor-owned and watcher-owned retry loops stay separate (D-59), the empirical
  cancel-and-resubmit finding that drives `reopen` (D-52), one `WatcherInner` with two tiers rather than a
  second watcher type (D-60), and why the M6.4 test seam is a private constructor rather than a
  feature-gated public one (D-64/D-65).

- [x] **M7.4** -- Opt-in, env-gated (`WINDOWS_FILE_WATCHER_STRESS`) stress suite in
  [tests/stress.rs](tests/stress.rs): 20,000-file change churn, a 25-round fault storm (repeated
  delete/recreate), 64 watchers tearing down concurrently under sustained churn, and 128 subscriptions
  coalesced onto one directory all receiving a change under load. Every test is enumerated and compiled by
  an ordinary `cargo test` but returns immediately unless the environment variable is set, so CI never pays
  its cost by default while still catching a build break.

- [x] **M7.5** -- Publication readiness: dropped "(in development)" from the crate description, added the
  `readme` field to `Cargo.toml`, and found and fixed a real gap during the review pass --
  `release-please-config.json` was missing the `crates/windows-file-watcher` package entry entirely (present
  in `.release-please-manifest.json` but not the config), which would have silently prevented a release PR
  from ever being generated for this crate. `CHANGELOG.md` is deliberately not hand-seeded: every other
  crate's was generated by release-please's own first release PR, and this crate has not had one yet.


## Moved 2026-08-21 -- M8: adopt wtf-string for relative names

- [x] **M8.1** -- Added the [wtf-string](../wtf-string/README.md) dependency and migrated `RelativeName`
  from its hand-rolled `Box<[u16]>` to `Wtf16Str`/`Wtf16String`, so decoded names carry the native-`u16`,
  conversion-free representation and feed Windows APIs without re-encoding, preserving the lossless
  `OsString`/`Path` and raw-`&[u16]` surface (D-8). Pulled forward out of dependency order (done after M2.3,
  since it is a representation change every later milestone touches).

- [x] **M8.2** -- Integration test ([tests/decode_real_buffer.rs](tests/decode_real_buffer.rs)): decode a
  buffer a real `ReadDirectoryChangesW` completion produced, for a file whose name contains an unpaired
  high surrogate (legal on NTFS, invalid Unicode). Asserts the raw `&[u16]` units match exactly, the
  `OsString`/`Path` conversions round-trip losslessly, and a direct wide-pointer (`as_wtf16().as_terminated_ptr()`)
  hand-off to a real Win32 API (`lstrlenW`) succeeds with no WTF-8 re-encode in between -- the same
  assertions M8.1's unit tests already made against a synthetic buffer, now against a real one.

## Moved 2026-08-21 -- M9: data-driven scenario stress

A load/stress suite whose scenarios are *data*, not one hardcoded test function per behavior: a scenario
is a value (an ordered list of filesystem operations plus timing parameters) that a single shared harness
executes and checks against the same generic invariants (no wedge, no panic, every desync is reported
rather than silently swallowed -- D-12). New scenarios are added by describing them, not by writing new
test bodies. Parameters (entity counts, wait-duration ranges, PRNG seed) are overridable so the same suite
can be run wider without code changes. Re-planned after M9.3: a scenario's *filesystem* actions (M9.1) are
only one dimension: a real client also opens and closes sessions and adds/removes watches while a
directory is live, and that lifecycle churn is a first-class part of the basic tier, not deferred to M9+
(M9+ is specifically the *concurrency* axis -- multiple threads, spoilers, nesting, queue overwhelm -- not
the single-threaded lifecycle churn M9.4/M9.5 add). This milestone covers only the single-modifier basics
the user asked to start with; concurrent modifiers, held-open "spoiler" handles, nested operations, and
queue overwhelm are explicitly deferred to M9+ once M9 is solid.

- [x] **M9.1** -- Data-driven scenario model: an `Operation` enum (create file, delete path, rename,
  make directory, wait) and a `Scenario { label, operations: Vec<Operation> }` (or builder) that a
  harness can execute mechanically -- no scenario-specific logic outside the data. Wait durations and any
  choice points are drawn from a small seeded deterministic PRNG (no external `rand` dependency needed;
  this crate has none today), defaulting to a fixed seed for reproducibility (per the repo's no-random-
  sampling-without-approval rule) with an env-var override to explore other seeds. Record the seeding
  decision in [DESIGN-NOTES.md](DESIGN-NOTES.md) with a new D-number.

- [x] **M9.2** -- Shared harness: given a `Scenario`, create a temp directory, subscribe a watch, apply
  every `Operation` in order (honoring `Wait`), and assert only the scenario-independent invariants: the
  watch never wedges (a liveness/notification deadline is always met while operations are still being
  applied), no panic, and every `Notification::Desync` is a reported loss rather than silence (D-12). The
  harness takes the scenario and its parameters (counts, timing ranges, seed) as arguments -- it has no
  hardcoded scenario knowledge. **Scaled for hundreds of thousands of operations per run (D-67):** the
  harness reports bounded per-kind tallies (`HarnessOutcome`), never a growing `Vec<Notification>`, and
  drains the queue non-blockingly after every operation so a long run never backs up the crate's own
  bounded queue between checks; `Operation::Repeat` keeps a large scenario's data small regardless of how
  many times it actually runs.

- [x] **M9.3** -- Basic scenario library, expressed as data through M9.1/M9.2: (a) a few files with a
  burst of changes, scaled up with `Operation::Repeat` to the hundreds-of-thousands-of-operations range a
  real stress run is expected to exercise; (b) delete-wait-reintroduce with irregular (PRNG-drawn) wait
  durations; (c) plain renames; (d) a directory created with the name a file used to occupy, and vice versa
  (cross-type name reuse); (e) a fast two-entity swap race: renaming file `x` -> `y` while concurrently
  (within the same operation batch, minimal or zero inter-op wait) renaming directory `z` -> `x`, to probe
  whether the two renames are ever misattributed to each other. **Found and fixed during this item (D-68):**
  applying real syscalls at hundreds-of-thousands scale is itself slow enough (~1,800 ops/sec measured) to
  trip the harness's fixed 120s timeout on throughput alone; `HarnessParams::for_operation_count` scales
  the budget from the scenario's own operation count so only a genuine stall still fails the assertion.

- [x] **M9.4** -- Session/watch lifecycle operations: extend the data model with operations that open and
  close *sessions* and subscribe and cancel *watches* mid-scenario (`Monitor::session` mints an independent
  channel per call -- D-2 -- so this is not a variation on M9.1's fixed single watch, it is a second kind of
  entity the harness must track by name and drain independently). Generalize the M9.2 harness from one
  fixed session/watch/receiver to a name-keyed table (`Fleet`, D-69) so a scenario can reference "the
  watch/session named X" from a later operation. Same generic invariants apply: no wedge, no panic, every
  desync counted; a session or watch that is already closed when an operation targets it is a
  scenario-authoring bug (assert), not a fault the harness tolerates -- unlike the M9+.2 "spoiler" case,
  which is deliberately about a live handle blocking an operation. **Delivered with both a delayed and a
  back-to-back timing posture (D-70):** session/watch churn spaced out with PRNG-drawn waits between every
  transition, and the same generator invoked with a near-zero wait bound for tight, continuous churn --
  because a fault or race is often a timing-window problem that only reproduces when transitions are
  spaced out enough to land mid-flight, not just under maximum throughput.

- [x] **M9.5** -- Persist the scenario model as JSON, as a real `run-scenario` CLI living in the crate
  itself (re-planned twice: first to add JSON persistence, then -- once the user chose a `[[bin]]` over a
  separate internal workspace crate -- to make `serde`/`serde_json` optional dependencies behind a
  default-off `scenario-tool` feature, D-72, since a `[[bin]]` target cannot draw on `[dev-dependencies]`
  the way `tests`/`examples` can). Delivered: `#[derive(Serialize, Deserialize)]` on `Operation`/`Scenario`
  (moved to `pub mod scenario` in `src/`, D-72) with `Duration` fields round-tripping through JSON as
  milliseconds; checked-in JSON fixture files under `tests/scenarios/` for the whole M9.3/M9.4 scenario
  library; a generic `every_persisted_json_fixture_runs_through_the_harness` test that walks every fixture
  and runs it through the harness; and `src/bin/run_scenario.rs`, a CLI taking a scenario JSON path as its
  one argument and printing the resulting `HarnessOutcome`. **The JSON schema is explicitly not part of
  this crate's semver contract (D-71)** -- by the engineer's own direction, since it is a testing/ops tool
  input, not a documented data format. Integration test for the milestone.

## Moved 2026-08-22 -- M9+: concurrent modifiers, spoilers, nesting, and queue overwhelm

Gated on M9 being solid: these widen the same data-driven model once the single-modifier basics
pass, per the user's own "start simple ... over time" sequencing. All four share one prerequisite --
`Fleet` moving behind a `Mutex` so `Operation::Concurrent`'s spawned threads can share it -- so they landed
together in one commit rather than four artificially separated ones.

- [x] **M9+.1** -- Concurrent modifiers: `Operation::Concurrent { branches: Vec<Vec<Operation>> }` runs
  every branch on its own thread (`std::thread::scope`, D-74), joining before the next top-level operation
  runs. Each branch draws its own PRNG seed from the parent before spawning, so a scenario stays
  reproducible for a given seed regardless of OS scheduling. Checked against the same no-wedge/no-panic/
  no-silent-loss invariants as any other operation.

- [x] **M9+.2** -- "Spoilers": `Operation::HoldOpen { path, duration }` opens a file without
  `FILE_SHARE_DELETE` (D-75) and holds it for `duration`, so a concurrent `Rename`/`RemoveFile`/`RemoveDir`
  targeting the same path fails with a real Win32 sharing violation -- verified by asserting the spoiled
  file is still present on disk afterward, not merely by notification counts.

- [x] **M9+.3** -- Nested operations: needed no new primitive. A `Concurrent` branch is an ordinary
  `Vec<Operation>` that may itself contain `Concurrent`, `Repeat`, or anything else in the model, so
  operation nesting (a rename racing an in-flight hold, a nested fork-join) falls out of D-74's design for
  free; demonstrated in `tests/scenarios/nested_concurrent_composition.json`.

- [x] **M9+.4** -- Queue overwhelm: `Operation::OpenSessionBounded { name, bound }` opens a session with an
  explicit, deliberately tiny queue bound (reusing `Monitor::session_with_bound`, D-76) instead of the
  crate's default, so a single large `Repeat` of churn -- applied before the harness's next drain pass --
  genuinely saturates it. The check is structural: the harness's own overall deadline is what would fail if
  backpressure ever became a wedge instead of a stall; the test deliberately does not assert on desync
  counts. Integration test for the milestone: `tests/scenario_stress.rs`'s three new M9+ tests plus four new
  JSON fixtures (`concurrent_file_churn.json`, `spoiler_blocks_delete.json`, `queue_overwhelm.json`,
  `nested_concurrent_composition.json`) all pass through the generic fixture runner.

## Moved 2026-08-25 -- M13: consumer test surface (test-util)

Let a downstream consumer drive *its own* notification-handling code with synthetic, deterministic
notifications -- no real filesystem, no thread pool -- by feeding the real [`Receiver`]. This is the
"go below" seam from the [testability design discussion](DESIGN-NOTES.md): the consumer substitutes the
OS ingest while keeping the crate's delivery model (`Notification`/`Receiver`/queue semantics) intact.
Because the consumer becomes the driver, their test is deterministic for free -- the crate ships no
scheduler or virtual clock.

The reachable part of the seam was already public (`WatchId::from_raw` and every boundary type); the
feedable channel (`channel_with_bound`/`Sender`/`Delivery`/`Reservation`) was `pub` only inside the
private `queue` module, hence unreachable, and is exposed behind `test-util` (discovered during
execution, revising D-81/D-82). Records D-81 (bless the already-reachable pieces rather than re-gate
them), D-82 (expose the feed channel and gap-filler constructors behind an off-by-default `test-util`
feature, reconciled with D-64 by audience), and D-83 (fidelity limit: the seam tests the consumer's
reactions, not whether the crate would ever emit that sequence).

- [x] **M13.1** -- Add an off-by-default `test-util` Cargo feature (no new dependencies) and record the
  three seam decisions D-81/D-82/D-83 in [DESIGN-NOTES.md](DESIGN-NOTES.md) (Tier 1) and
  [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) (Tier 2).

- [x] **M13.2** -- Fill the `RelativeName` gap behind `test-util`: valid-by-construction `for_test`
  constructors from `&str`/`&OsStr`/raw `u16` units, so a consumer can build a `Change`. Unit test.

- [x] **M13.3** -- Fill the `VolumeIdentity` gap behind `test-util`: promote the `#[cfg(test)]`
  `synthetic` builder to a `test-util`-gated public `for_test` constructor, keeping the crate's own
  `#[cfg(test)]` use working. Unit test.

- [x] **M13.4** -- Expose and document the consumer test surface: re-export
  `channel_with_bound`/`Sender`/`Delivery`/`Reservation` behind `test-util` (they were `pub` only inside
  the private `queue` module, hence unreachable -- revised D-81/D-82); rewrite `WatchId::from_raw`'s
  stale doc; frame `channel_with_bound` + `Sender::send` as the injection seam; add a crate-level
  "Testing your consumer code" docs section with a deterministic, cfg-gated doctest (D-83 fidelity limit).

- [x] **M13.5** -- Consumer-facing example [examples/test_your_handler.rs](examples/test_your_handler.rs) (`required-features =
  ["test-util"]`): a small handler driven by a scripted deterministic sequence pushed through
  `channel_with_bound`/`Sender::send` -- covering `Batch` (gap-filled `Change`), `Desync`, `Completion`,
  `RetryQuestion`, and `VolumeChanged` (gap-filled `VolumeIdentity`) -- asserting the handler's
  reactions, with no filesystem and no thread pool.

- [x] **M13.6** -- Integration test [tests/consumer_test_surface.rs](tests/consumer_test_surface.rs) (`required-features =
  ["test-util"]`) exercising the surface exactly as a downstream consumer would (public + `test-util`
  items only): a scripted sequence covering every `Notification` variant including both gap-filled types,
  asserting deterministic receipt through `Receiver`.

## Moved 2026-08-27 -- M14: audit the delivery contract against the ten specification-gap categories (D-84)

PR #42 found gaps in this crate's contract prose one at a time, reactively, over 19 review rounds. M14 ran
the converse pass: a deliberate sweep asking, for each of
[the ten categories](../../DESIGN-NOTES.md#specifying-a-delivery-contract), whether this crate states the
answer or leaves it to omission. It found four shipped defects and five rules that were true of the code and
written down nowhere. Full results in [The M14 audit](DESIGN-NOTES.md#the-m14-audit) and
[The M14.3 predicate sweep](DESIGN-NOTES.md#the-m143-predicate-sweep).

- [x] **M14.1** -- Audited D-12 (`Desync`), D-27/D-28 (the fault protocol), and D-30 (request completions)
  against all ten categories. Three documentation defects: `DesyncCause`'s type-level doc claimed every cause
  is advisory and always answered by a re-scan while its own `Stopped` variant said the opposite (a consumer
  reading the type doc first would re-scan forever against a dead watch); "The Desync primitive" enumerated
  four causes when there are five; and "Delivery and saturation" still described the pre-D-29 drop policy and
  the exact latch phrasing D-39 corrects. Two rules stated for the first time: the standing slot shared by
  `RetryQuestion` and `VolumeChanged` rests on a mutual-exclusion invariant its soundness depends on, and
  D-30's "every request produces a completion" holds for lifecycle requests only -- `Answer` and
  `AnswerVolumeChange` deliberately carry none.

- [x] **M14.2** -- Audited D-10, D-13, D-17, D-26 and D-57 the same way, folding every newly-stated rule into
  the harness's `schedule` module docs. Three legal sequences the contract described in a way that excludes
  them: a liveness bracket can open with `Resumed` (a route coalescing onto an already-faulted watcher joins
  after `enter_fault` sent its `Suspended`s) and can close with `Desync { Stopped }` instead of `Resumed`, so
  a consumer balancing brackets is wrong in both directions; `Established` is not necessarily a watch's first
  notification; and the tier is re-resolved on every reopen (D-61), so it may differ between establishments.
  Also recorded that D-10's "one completion = one batch" is per-subscription and only when that
  subscription's filtered subset is non-empty.

- [x] **M14.3** -- Swept all nine advisory predicates this crate exposes or consumes for the `has_room`
  shape. One defect: `Receiver::is_empty` is `len() == 0`, and `len` excludes owed latched losses, so a
  drained queue that still owes a `Desync { QueueFull }` reports itself empty while `recv` would still yield
  it. D-41 makes that a spin rather than a silent miss -- the doorbell is signalled on all three of queued,
  owed, and disconnected, so a client that waits on it and then tests `is_empty` never collects the report it
  was woken for. That this crate's own tests never use `is_empty` alone, always rebuilding
  `!is_empty() || latched() > 0` by hand, is the same signal `has_room` gave before it was fixed. Fixed by
  publishing D-41's own predicate as `Receiver::has_pending` rather than redefining `is_empty`, with three
  regression tests that had no predecessor.

## Moved 2026-09-01 -- M15.6: bounded waiting across queue/tests.rs

### <a id="m156"></a>M15.6 -- Convert `queue/tests.rs` to bounded waiting, so a broken wake fails instead of hanging. *(completed 2026-09-01 17:37:46 -04:00)*

**The sweep.** 40 unbounded `recv()` sites reduced to 1 -- the survivor is inside `assert_stream_ended`
itself, where the bound is the assertion. All 40 were the same `receiver.recv().expect(...)` shape on the
same variable, so the transform was mechanical; the one nearby `recv()` at line 1069 is an unrelated
`mpsc` receiver and was left alone. The transform cannot turn a passing test red, which is why it needed
no staging: a test that was already receiving what it expected still receives it within the bound.

**The result, measured.** A confirming full-file sweep after the change: **124 mutants in 20 minutes,
78 caught / 8 missed / 14 timeout / 24 unviable**, against a pre-sweep baseline of 32.9 minutes.
The two mutants that previously hung for 120s each are now **caught in 32s**.

**The finding that changes how `timeout` should be read.** Every one of the 14 remaining timeouts had
between **4 and 132 tests already `FAILED`** before cargo-mutants killed the run. None is a wedge and
none is a gap: the mutant is detected, and the run is killed only because a suite in which dozens of
bounded waits each burn their budget exceeds 3x the baseline. Bounded waiting converted the remaining
hangs into detected failures; what is left is a scoring artifact, and `missed` is now the only column
that indicates a real gap. The cost is real, though -- 14 x 67s is 15.6 minutes of the 20-minute run --
and the lever on it is the test-side wait budget, not more test work (see M15.7).

**Gaps closed while adjudicating the residue** (each verified by re-injecting the mutant and observing
a red suite, never by reasoning):

- `freed_resumers`' `!took_one ||` guard. `&&` binds tighter than `||`, so the mutant reads
  `(!took_one && resumers.is_empty()) || room != 1`, which differs only when a take that took *nothing*
  lands on the edge -- `Receiver::try_recv` is the sole caller that can report `took_one == false`. The
  mutant makes an empty poll prod a parked producer, so a producer behind a saturated queue would be
  woken by pollers rather than by capacity, and woken with no room to use. Closed by
  `a_take_that_took_nothing_is_not_a_crossing_and_does_not_prod`.
- `Debug for Receiver`, both an empty body and an inverted `disconnected` flag. This impl is the only
  outside view of the queue's occupancy and is exactly what someone diagnosing a stalled watcher reads;
  an inverted flag is worse than no flag, because it misleads at the moment it is consulted. Closed by
  `a_formatted_receiver_reports_the_state_a_wedge_is_diagnosed_from`, which asserts the rendered string
  whole.
- `Debug for Sender`, replaced with a body that writes nothing. Folded into the existing
  `the_opaque_handles_name_themselves_when_formatted`, which already carried this exact rationale for
  `StandingSlot` and `Reservation`.

The remaining 4 missed mutants are all in `StandingHold::drop`, whose body is unreachable; that is M15.1
and is an engineer decision, not a test gap.
## Moved 2026-09-01 -- M15.1: the unreachable half of `StandingHold::drop`

### <a id="m151"></a>M15.1 -- Decide whether `StandingHold::drop`'s release path is dead code, and act on the answer. *(completed 2026-09-01 18:29:36 -04:00)*

**The reachability question, settled from the source.** Every `StandingHold` is built in one place
(`StandingSlot::send`) and moved straight into `state.queue`; `Entry::plain` never carries one. The only
site anywhere in the crate that removes an entry from that queue is `take` (grepped for
pop/drain/retain/clear/remove/truncate -- one hit), and it settles the reservation inline with the pop and
sets `resolved`, so `Drop` returns at its first line. The only other way a hold dies is `Shared` being torn
down, where the `Weak` fails to upgrade and it returns at its second. Confirmed empirically by replacing the
body with `unreachable!()` and passing the full `--all-features` suite (372 tests, lib + 8 integration
targets + doctests).

**The history question, settled from git.** The checklist offered two readings; the first is disproved.
`4198aa8` says this `Drop` "unconditionally restored `reserved` **on drain**" -- it *was* the drain path.
`07d4b75` (PR #20 review 5000746684) then found that popping exposed the queue slot before the deferred
`Drop` restored the reservation, so `queue.len() + reserved` could exceed capacity; the fix moved the
release into `take` and left `Drop` as "the fallback for every other discard." It was live code whose only
caller moved out from under it, not scaffolding that was never wired up.

**The finding that decided the outcome: the body could not have run safely.** `take` takes `&mut State`, so
its caller holds the `items` guard -- and any other way to remove an entry from `state.queue` needs that
same guard, so a hold discarded on such a path is dropped *inside* the lock. The body's first act was
`lock(&shared.items)`, a plain non-reentrant `Mutex::lock`. Differential measurement, identical forced
unwind out of `take`: body live -> **hung past 90s**; `Drop` short-circuited -> **exit 101, immediate**. The
"fallback for every other discard" would have deadlocked in exactly the situation it was written for.

**Building the discard path was never an option.** It would contradict a tested decision:
`dropping_a_standing_slot_while_its_message_is_still_queued_releases_capacity_once` asserts that a cancelled
slot's queued question **still arrives**.

**What landed.** The body is replaced by `debug_assert!(std::thread::panicking(), ...)` -- the true
statement rather than a bare `false`, so the one way to arrive today (an unwind out of `take` between the
pop and `resolved`) lets the original panic propagate instead of becoming an abort from a second one, while
any other arrival fires. It encodes the contract the deadlock taught: a discard must release the reservation
under the `items` lock it already holds, exactly as `take` does. `a_hold_that_outlives_its_entry_while_the_queue_is_alive_trips_the_tripwire`
exercises it, because an assertion nothing exercises is worth no more than the comment beside it.

**Mutation result.** All four survivors are gone -- three by deletion, and the whole-impl mutant
(`replace drop with ()`) is now caught by the tripwire test. Every branch the new `Drop` admits was injected
and confirmed caught: `resolved` -> `true`/`false`, `upgrade().is_none()` -> `true`/`false`, and the empty
body. `queue.rs` now has **4 missed mutants, none in this impl**.

**Blast-radius sweep.** The survivors were the symptom of a doc gone false by vacuity: `Entry`,
`StandingHold`, `StandingState`, and `take` all described this `Drop` as the live release mechanism. Four
restatements of one fact, none of which moved when the fact did; all four corrected here, plus the note in
`queue/tests.rs`. Recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `Dead code that could not have run`.
## Moved 2026-09-01 -- M15.2 / M-inf.4: why a by-id reopen cannot be watched

### <a id="m152"></a>M15.2 -- Explain, then either fix or document, why a handle from `reopen_by_id` rejects the very read the watcher exists to issue. *(completed 2026-09-01 19:05:00 -04:00)*

Closes **M-inf.4** as well, which had parked exactly this root-cause question.

**Root cause, measured with a control.** A standalone probe -- no thread pool, no IOCP, no crate code,
just `CreateFileW`, `OpenFileById`, `NtCreateFile` and a bare `ReadDirectoryChangesW`:

| How the directory was opened | `ReadDirectoryChangesW` |
|---|---|
| `CreateFileW`, by path | TRUE (pending) |
| `OpenFileById` | FALSE, `ERROR_INVALID_PARAMETER` (87) |
| `NtCreateFile` by id, **with** `FILE_DIRECTORY_FILE` | FALSE, 87 |
| `NtCreateFile` by id, without `FILE_DIRECTORY_FILE` | FALSE, 87 |
| **control:** `NtCreateFile` **by name**, same options | TRUE (pending) |

All five handles are identical everywhere it is natural to look: `FileModeInformation` reports each
asynchronous (neither `SYNCHRONOUS_IO` bit set), `FileAccessInformation` reports the same granted mask
`0x00100081`, and `FileNameInformation` resolves the same path. The only variable that changes the
outcome is whether the object was resolved **by file ID** or **by name**.

**The checklist's three readings, adjudicated.** (b) is disproved -- it is not `SYNCHRONIZE` or
volume-hint semantics, since access and mode are byte-identical. (c) is disproved by the by-name control
through the identical `NtCreateFile` call. It is (a): the reopen path cannot serve a watch.

**D-80's recorded cause was wrong, which mattered.** It attributed the failure to "the `OpenFileById`
handle's interaction with IOCP association/arming specifically" and recorded the cause as not understood.
IOCP is not involved at all. An attribution recorded as unexplained is not inert -- it had named a
subsystem, and naming the wrong one is worse than naming none, because it points the next investigation
away from the answer.

**Confirmed against the crate.** Re-enabling the fast path fails six tests, every one a fault-resolution
test, with "the fault never resolved after being answered" -- D-80's symptom verbatim. Instrumenting the
arm prints `Os { code: 87, kind: InvalidInput }`. The read never completes because it never starts: the
arm fails, the watcher re-faults, retries, reopens by id again, and fails identically.

**Removed, not disabled.** It is impossible rather than blocked, and it could not have paid for itself
either way: `reopen_via_existing_handle` returned its candidate only when the reopened object's path
already equalled the watcher's recorded canonical path, so by construction it could only ever hand back a
handle to the object *at the path the path-based fallback already opens*. Gone:
`WatcherInner::reopen_via_existing_handle`, `DirectoryHandle::reopen_by_id`,
`DirectoryId::file_reference`, and the four `reopen_by_id_*` identity tests that characterised the
mechanism.

**What replaces them.** [tests/reopen_by_id_cannot_be_watched.rs](tests/reopen_by_id_cannot_be_watched.rs)
asserts the OS limitation itself, including the by-name control, so the decision rests on something that
executes rather than on a paragraph. If a future Windows accepts that read, the test fails and D-80 should
be revisited.

**The surviving mutant is explained rather than closed.** `directory.rs`'s `|` -> `&` in `reopen_by_id`
zeroed both flags; measurement shows `OpenFileById` on a directory succeeds with **flags = 0**
(`FILE_FLAG_BACKUP_SEMANTICS` is not required for a by-id open the way it is for `CreateFileW`), and the
only difference is that the handle becomes synchronous. That property is observable exclusively through an
I/O this handle can never perform -- so the mutant was unkillable, and it is now moot: the function is
gone.

**Blast-radius sweep.** D-80's decision row and detail section, the reopen paragraph above it, the field
and method docs on `WatcherInner::directory_id` / `canonical_path` / `DirectoryHandle::canonical_path`,
`retry_reestablish`, `on_path_based_reopen`, a `monitor::tests` comment, and the M11.1/M11.2/M11.5
checklist entries all described the fast path as live or as pending root-cause; all corrected.
`COMPLETED-PLANS.md` is left alone as dated, append-only history.

**Spawned M15.8:** `WatcherInner::canonical_path` is now write-only, and no warning will ever surface it
because `lock(&self.canonical_path)` counts as a read of the field.
## Moved 2026-09-01 -- M15.8: the write-only tail of M15.2's removal

### <a id="m158"></a>M15.8 -- Decide whether `WatcherInner::canonical_path` and `DirectoryHandle::canonical_path` survive M15.2's removal. *(completed 2026-09-01 19:20:00 -04:00)*

**The finding that reframed the question.** M15.8 was written as remove-both or keep-both, because the
field and the method looked like one unit. They are not: the only reader worth having reads the **new
handle**, not stored state. So the field and the method were settled separately, and the answer is
different for each.

**The field is gone.** `WatcherInner::canonical_path` was written on every install and read by nothing --
its one reader was the by-reference fast reopen D-80 removed. No compiler warning would ever have surfaced
it, because `lock(&self.canonical_path)` counts as a read of the field: the same invisible-dead-code shape
as M15.1's unreachable `Drop`.

**The method stayed, and gained a caller that uses its result.** The "reopened on a different volume than
before" warning printed `self.path` -- the client-supplied string that `WatcherInner`'s own doc comment
calls "possibly not even fully resolved". That is the one moment that string is least worth printing,
because changed resolution is exactly what happened. It now names the path the handle *resolves to*, with
`None` itself informative (the new handle would not report a path at all).

**It had no tests at all.** The four `reopen_by_id_*` tests were `canonical_path`'s only coverage and went
with M15.2, so keeping it meant keeping an untested Win32 helper. Two tests added: that it reports where
the handle actually is (compared against the OS's own answer, not against the opening string), and that a
fresh query follows a rename -- being fresh rather than cached is the entire reason it exists, and a
cached implementation would pass a weaker test.

**Verified by injection, and the boundary with M15.3 confirmed.** `buffer.truncate(written)` ->
`truncate(written + 1)` is now **caught**; `written < buffer.len()` -> `<=` still **survives**, which is
M15.3's mutant exactly -- it needs a 512+ unit canonical path to reach. So M15.3 is left standing and
unanswered rather than dissolved, which is what this item had to determine.

**The transferable rule:** a diagnostic wants the live handle, not a cached copy. Needing the *value* is
not a reason to keep the *field*.
## Moved 2026-09-01 -- M15.3: paths are the caller's, verbatim (D-85)

### <a id="m153"></a>M15.3 -- Decide whether this crate should open paths longer than `MAX_PATH`, and note the consequence for `canonical_path`'s retry either way. *(completed 2026-09-01 19:45:00 -04:00)*

**Decision: D-85 -- a caller's path reaches Win32 verbatim, and long-path support is the consuming
application's call, not this crate's.** That is what the code already did; what changed is that it is now
a stated decision with the measurements behind it, rather than an unexamined default.

**The item's own proposed fix was measurably wrong.** M15.3 offered "prefix `\\?\` in `wide_path`" as a
coherent outcome. `\\?\` is a path *parsing mode*, not a length switch, and adopting it on a caller's
behalf changes what their path means -- measured on a **short** directory that opens fine today, so this
is nothing to do with length:

| what the caller passed | verbatim | prefixed |
|---|---|---|
| `C:/Users/.../dir` (forward slashes) | opens | `ERROR_FILE_NOT_FOUND` |
| `C:\...\dir\.` | opens | `ERROR_INVALID_NAME` |
| `C:\...\dir\subdir\..` | opens | `ERROR_INVALID_NAME` |

Relative paths would stop resolving entirely, and this crate supports them deliberately --
`open_file_target` normalises a bare leaf's empty `parent()` to `.` precisely so `subscribe("target.txt")`
works.

**The item's premise was measuring the harness, not the crate.** M15.3 recorded that a directory deeper
than `MAX_PATH` "cannot be opened". Long-path support without the prefix needs the machine's
`LongPathsEnabled` policy **and** the application's `longPathAware` manifest. On this machine the policy
was already `1`; the same probe source, same machine, differing only by an embedded manifest:

| build | verbatim 300-character path | `\\?\` form |
|---|---|---|
| no manifest | `ERROR_PATH_NOT_FOUND` | opens |
| + `longPathAware` manifest | **opens** | opens |

A library cannot set its consumer's manifest, and a consumer that has not opted in should not have this
crate opt in behind its back. A Rust test binary has no such manifest, which is the whole reason this
looked like a crate defect.

**The traversal hazard does not arise here, and the decision says so rather than leaving it silent.**
Win32 has no relative open, so traversal must *build* child paths that can exceed `MAX_PATH` even when the
caller's did not -- the one case where the caller's parsing mode is genuinely not enough. This crate never
lengthens a path: recursion is the kernel's (`bWatchSubtree`), names stay relative (D-8), and the only
structural path operation in production code is `open_file_target`'s `parent()`, which shortens. So the
decision **schedules no work** for traversal; it records the rule for if it ever appears -- build on
`DirectoryHandle::canonical_path`, not the caller's string, because `GetFinalPathNameByHandleW` returns the
`\\?\` form *after* Win32 has applied the caller's parsing mode, making the switch meaning-preserving.

**One consequence checked rather than assumed:** because the canonical form is a different parsing mode
from the caller's, mixing them in a comparison would be a bug. Nothing does -- `opened_path` is stored
verbatim and only ever reopened, never matched against a canonical path.

**Deferred deliberately, as work items rather than prose:** M15.9 (guard tests pinning the pass-through, so
a future "helpful" prefix fails the suite) and M15.10 (the junction fixture for the 512-unit retry -- the
back door M15.3 called hypothetical is confirmed to work, no elevation needed, a 53-character junction
resolving to a 578-character target).
## Moved 2026-09-01 -- M15.9: guarding D-85's pass-through

### <a id="m159"></a>M15.9 -- Guard D-85's pass-through with tests, so a future "helpful" `\\?\` prefix fails the suite instead of silently changing what callers' paths mean. *(completed 2026-09-01 20:05:00 -04:00)*

**Five guards, in a labelled `directory::tests` section.** Four new -- forward slashes, a `.` component,
a `..` component, and a caller's own `\\?\` path -- plus `opens_the_current_directory_by_relative_path`,
which already existed and turned out to be load-bearing for the same reason; its comment now says so, so
it is not simplified away by someone who does not know.

**Each asserts the resolved identity, not `is_ok()`.** A path that opens the *wrong* directory is the
failure worth catching, and the `..` case is the one where that matters concretely: under an ordinary
parse it must land on the parent, and a test that only checked "something opened" would accept a handle
on the child.

**The measurement that justifies the item.** With a blanket prefix injected into `wide_path`, **exactly
those five fail and the other 33 tests in the module pass.** So a "helpful" prefix would otherwise land
looking entirely green -- the existing suite does not constrain this at all, which is precisely why the
guards had to be written rather than assumed.

**Two facts settled by that run, rather than by reasoning.** A trailing separator *survives* the prefix
(`opens_a_directory_with_a_trailing_separator` passed), so it is not a distinguishing case and was left
out. And the guards are precise rather than broad: they fail on the prefix specifically, not on
incidental path handling.

**Deliberately absent: a long-path test.** A Rust test binary has no `longPathAware` manifest, so a
`MAX_PATH`-exceeding open fails in-suite regardless of machine policy; asserting that would pin the
harness rather than the crate. That is stated in the section header so the omission reads as a decision
rather than an oversight. M15.10 covers the part that *can* be tested.

**A self-inflicted detour worth recording.** The first injection was written through PowerShell and
over-escaped -- eight backslashes reached Rust where four were needed -- so every path became invalid and
23 tests failed instead of 5. The over-broad result is what exposed it; a subtler mis-escape would have
read as a real finding. Redone with a direct file edit and a Rust raw string, which removes the escaping
layer entirely. This is the same multi-layer escaping trap recorded earlier in this sweep.
## Moved 2026-09-01 -- M15.10: covering canonical_path's regrow

### <a id="m1510"></a>M15.10 -- Test `canonical_path`'s 512-unit retry through the junction back door. *(completed 2026-09-01 20:25:00 -04:00)*

**The item's own premise was wrong, and checking it first saved the whole fixture.** M15.10 (and M15.3
before it) held that the retry was unreachable through the crate's API, so a junction pointing at a deep
target was the only way in. But D-85's pass-through means a caller's own `\\?\` path opens past
`MAX_PATH` *without* the host carrying a `longPathAware` manifest -- which M15.3's own probe had already
shown. So `DirectoryHandle::open` on a long `\\?\` path reaches the retry directly: no junction, no
reparse-point plumbing, no spawned `mklink`, and no question about elevation.

**Two tests.** One drives a ~560-unit path through `open` and asserts the grown buffer carries the whole
path rather than a truncated one. The second walks **every length from 508 to 516 units**, sizing each
fixture exactly by padding its final component, so the boundary itself is pinned: 511 units is the last
that fits one call and 512 the first that needs the regrow, and an off-by-one on either side would leave
a path truncated or looping.

**The branch really was uncovered.** Verified by making the regrow return an error: **exactly one test
failed and 38 passed.** Nothing else in the module reaches that code.

**Two claims from M15.3 corrected by measurement.** Its `<` -> `>` mutant does *not* loop forever -- it
is caught and fails fast. And `<` -> `<=` is not a gap but a genuinely **equivalent** mutant: Win32's
two-call convention makes `written == buffer.len()` unreachable, because success returns the length
excluding the NUL (needing room for it) while a too-small buffer returns the length including it. That
was confirmed rather than argued -- an `assert_ne!` probe never fired across the whole suite, including
the 508-516 walk that straddles the buffer size exactly. The reasoning is now a comment at the
comparison, so the next sweep does not re-litigate it.
## Moved 2026-09-01 -- M15.4: isolating the last notification-filter categories

### <a id="m154"></a>M15.4 -- Isolate the last two notification-filter categories, or record that they cannot be isolated from outside. *(completed 2026-09-01 20:35:00 -04:00)*

**They can be isolated, and the item's reasons for thinking otherwise were both wrong.** The item did the
right thing in naming the measurement it needed -- arm a watch with a single filter bit at a time and see
which fires -- and doing that measurement overturned its own premises.

| operation | which bit reported it |
|---|---|
| `SetFileTime` last-write only | **LAST_WRITE alone** |
| `SetFileTime` creation only | **CREATION alone** |
| `SetFileTime` last-access only | LAST_ACCESS alone |
| DACL edit (`icacls`) | **SECURITY alone** |
| same-length rewrite | SIZE and LAST_WRITE -- **not** ATTRIBUTES |

**Correction 1: the archive bit was a red herring.** The item held that a same-length rewrite is masked
because it sets the archive bit and ATTRIBUTES reports it. ATTRIBUTES does not fire for that operation at
all. What actually reports it is SIZE, because `std::fs::write` truncates before writing, so the length
genuinely does change on the way through even though the net length is the same.

**Correction 2: the DACL test already worked.** The item held that a DACL edit is reported through "some
filter this exercise did not identify" with SECURITY dropped. A DACL edit fires SECURITY and nothing else,
and injecting the SECURITY-dropping mutant turns `changing_a_files_permissions_is_reported_as_modified`
red. That mutant was already closed; the record said otherwise because the earlier check was not
measuring what it claimed.

**What was added.** Two tests using `SetFileTime`, one touching only the last-write stamp and one only the
creation stamp. The handle is opened for `FILE_WRITE_ATTRIBUTES` **alone**, not for writing, because a
write-access handle can set the archive bit on close and would quietly undo the isolation. The
creation-time test is the interesting one: the surviving mutants drop *adjacent pairs*
(LAST_WRITE+CREATION and CREATION+SECURITY), so a creation-only change is the single operation that fails
under both.

**Result: all six flag-pair mutants in `ALL_NOTIFY_FILTERS` are now caught**, each confirmed by injecting
it and watching the module go red -- including the four the item had assumed were already covered, which
had never been verified individually.

**A harness lesson, since the first probe produced two confidently wrong tables.** Reusing one directory
handle across probes makes every read complete instantly against change records buffered from earlier
operations -- the first run reported nothing firing, the second reported everything firing. Both were
harness artifacts. The fix is a fresh handle per probe plus decoding the returned records and matching by
file name, and a built-in sanity check (a known operation under a known bit) so a broken harness announces
itself instead of producing a plausible table.
## Moved 2026-09-01 -- M15.5: the arming contract, asserted instead of crashed into

### <a id="m155"></a>M15.5 -- Assert the arming contract in `arm_detailed_read`, so a broken one fails a test instead of *sometimes* corrupting the heap. *(completed 2026-09-01 20:45:00 -04:00)*

**The shape of the problem.** The submission closure classified `ReadDirectoryChangesW`'s result inline,
so nothing could observe that classification except by running a real watcher and seeing what the pool
did with the result. Getting it wrong tells the pool to reclaim an I/O the kernel will still complete, and
the completion lands in a freed buffer -- detection then depends on allocator behaviour rather than on any
assertion. The same mutation was recorded `MISSED` in one sweep and a crash in another, and two crashes
were scored `CaughtMutant` purely because the process exited non-zero.

**The fix: make the classification a value, not a side effect.** `classify_submission` is now a free
function taking the raw result and a lazily-taken `GetLastError`, returning `Result<Issued, io::Error>`.
Four tests assert the whole contract -- a non-zero return is still pending (a packet is coming even though
the I/O finished), `ERROR_IO_PENDING` is pending, a genuine failure is a failure carrying the OS code, and
an error with no OS code falls through to failure rather than matching the pending comparison. The success
test also passes a `last_error` closure that panics, pinning the rule that `GetLastError` must not be read
after a success, where it holds a stale value from some unrelated call.

**The detail that made the first attempt insufficient, and why it is worth recording.** The extraction
initially took a `bool`, leaving `ok != 0` at the call site. Injecting *that* comparison was still "caught"
only by the process dying with `STATUS_HEAP_CORRUPTION` (exit `0xC0000374`) -- the exact failure mode this
item exists to remove, simply relocated by one operand. The function now takes the raw `BOOL` and does the
`!= 0` itself, so the whole convention sits inside the tested surface and the call site has no comparison
left to get wrong. Verifying the fix rather than assuming it is what surfaced this.

**Result.** Every mutant in the arming classification -- `!= 0` to `== 0`, to `true`, to `false`; the
pending equality to `!=`; and the failure arm to `Ok(Pending)` -- now fails with exit 101, a clean test
failure, in about a second each. None crashes, so the file's mutation score stops depending on heap
layout.

**Ruled out beforehand, and left recorded so nobody repeats it: the shipping code was never at fault.**
55 runs of the unmutated suite (25 default-feature, 30 `--all-features`, including the exact binary named
in every crash report) produced zero failures, and all sixteen crash reports carry distinct PE timestamps,
none equal to the clean build's -- each was its own mutant build. The test binary's filename derives from
target and features rather than contents, which is why every report names the same `.exe` and why that
name alone proves nothing.
## Moved 2026-09-01 -- M15.7: the test-side wait budget

### <a id="m157"></a>M15.7 -- Decide the test-side wait budget, so a mutation sweep is not dominated by tests that correctly fail slowly. *(completed 2026-09-01 21:05:00 -04:00)*

**Decision: `NOTIFY_TIMEOUT` lowered from 30s to 5s**, in all three copies (`src/watcher/tests.rs`,
`tests/fault_detail.rs`, `tests/watched_paths.rs`). The item framed this as a trade against flake
resistance that only the engineer could make, and it is -- but it was being made without any measurement
of what the budget was actually absorbing. Getting that first turned a taste call into an arithmetic one.

**What the waits cost.** Every `wait_until` was instrumented and the suite run three ways:

| condition | median | p95 | max |
|---|---|---|---|
| idle | 0 ms | 2.5 ms | 506 ms |
| + a full release build | 25 ms | 94 ms | 509 ms |
| 3x concurrent suites + a build | 0 ms | 2.5 ms | 508 ms |

**45 of 46 waits complete in 2.5ms or less**, and the entire tail is a single test gated on the retry
*backoff timer* at ~515ms -- structural, not notification latency. The numbers barely move under 4x
oversubscription on a 12-core machine, which is the finding that mattered: the 30s budget was absorbing
no contention it needed to.

**What lowering it buys, measured on one of the mutants that had been scored `timeout`** (changing only
`NOTIFY_TIMEOUT`; a first attempt also raised the unrelated 5s bounds and had to be redone):

| budget | suite time | cargo-mutants verdict (67s kill) |
|---|---|---|
| 30s (as shipped) | 93.6s | TIMEOUT -- killed |
| 10s | 42.1s | caught, a clean red test |
| 5s | 31.8s | caught, a clean red test |

That is the whole of the item's 15.6-minutes-of-20 problem: 14 mutants x 67s, every one already detected
(between 4 and 132 tests had failed before the kill) but filed as `timeout` rather than `caught`.

**Stability check, since margin was deliberately traded away.** Six consecutive full-suite runs under
sustained concurrent release builds: 6/6 green.

**The residual risk, recorded rather than smoothed over.** All of this came from one 12-core developer
machine. 5s is ~10x the structural outlier and ~2000x the p95, but a slower runner -- a 2-core CI box, or
one with antivirus scanning temp directories -- is unmeasured. The constant's doc comment says so, and
says the answer to a flake there is to raise the number, not to doubt the tests.

**An option the item did not list, and why it was not taken.** An environment-variable override (default
30s, sweeps opting into 5s) would have avoided transferring any risk to CI. It was offered and declined in
favour of the simpler global change -- worth recording, because it is the fallback if the residual risk
above ever materialises.
**Confirmed end to end by a real sweep of `queue.rs`**, against the run that motivated the item:

| | before (30s budget) | after (5s budget) |
|---|---|---|
| caught | 78 | **89** |
| missed | 8 | **0** |
| timeout | 14 | **8** |
| wall clock | 20 min | **14 min** |

(`missed` reaching 0 is M15.1 and M15.8's doing, not this item's.) The six mutants that stopped timing out
moved into `caught`, exactly as the arithmetic predicted.

**What the remaining 8 turned out to be -- a different defect, now M15.11.** All 8 still had between 4 and
76 tests failed before the kill, so they remain detections rather than gaps. But none is held by a
notification budget: every one is pinned by a single test,
`queue::tests::no_wakeup_is_lost_under_a_concurrent_burst`, whose *wait* is bounded while the **loop around
it is not**. Each of those mutants breaks that loop's exit condition, so it spins at full speed rather than
failing. That is M15.6's shape one level up, and M15.6 missed it because the bound sits on the wait rather
than on the loop -- lowering the budget further would not have touched it.
## Moved 2026-09-01 -- M15.11: a bounded wait inside an unbounded loop

### <a id="m1511"></a>M15.11 -- Bound the *loop* in `no_wakeup_is_lost_under_a_concurrent_burst`, not just the wait inside it. *(completed 2026-09-01 21:25:00 -04:00)*

**The shape, which is the transferable part.** The test waited on the doorbell with a bounded
`await_signal`, drained, then re-checked an exit condition of
`is_disconnected() && is_empty() && latched() == 0`. Every one of those calls is an answer a defect can
pin -- and a pinned answer leaves the doorbell *signalled*, so the bounded wait returns immediately and
the loop spins at full speed. **A bounded wait inside an unbounded loop is still an unbounded loop**, and
that is exactly why M15.6 did not find this: it swept for unbounded `recv()`, and here the `recv` was
already bounded.

**What it cost.** All 8 timeouts in M15.7's confirming sweep were this one test, held open by eight
different mutants (`try_recv -> None`, `recv`'s `==` to `!=`, `is_disconnected -> false`, `len -> 1`,
`is_empty -> false` twice, `latched -> 1`, `take -> None`). Each was a detection -- between 4 and 76 tests
had already failed -- filed as a timeout because the run was killed before it could finish.

**The fix.** A deadline on the loop itself, whose failure message names `seen`, `TOTAL`, and all three
predicate values, so a future instance says which answer got pinned rather than just that time ran out.
Measured on four of the eight mutants: each now fails in seconds where it previously cost the full 67s
kill.

**Also aligned:** `await_signal` still waited 30s, the budget M15.7 had just lowered everywhere else. It
is now 5s, with the reasoning cross-referenced rather than restated.

**The suite-wide sweep the item asked for found no second instance.** Ten candidate loops were flagged
mechanically and all but this one are sound: three in `watch/tests.rs` bound themselves with a
`!remaining.is_zero()` assertion (an idiom the first heuristic did not recognise), one terminates because
the bounded helper it calls *panics* rather than returning, two are path-construction loops that terminate
by growth, and the rest are drain loops or stop-flag threads the test controls.

**Worth recording about the search itself:** the first heuristic -- "a loop with no deadline in its body"
-- returned a false negative on *this very test*, because `await_signal` appears in the body and looked
like a bound. The check that works is narrower: does the loop assert against a **deadline it owns**, as
opposed to merely calling something that is itself bounded.
**A near-miss worth recording, because only injection caught it.** The first fix for the remaining
`Receiver::recv` timeout was to replace the three bare `assert!(receiver.recv().is_none())` sites with
`recv_timeout`. The suite stayed green and the hang went away -- and the mutant went from `timeout` to
**SURVIVED**. Those assertions are the only thing that exercises `recv`'s *own* disconnect check, so
bounding them by swapping the call stopped testing the very path they existed for: a detection had been
traded for a gap, invisibly, while every surface signal said the change was an improvement.

The correct bound keeps the blocking `recv()` and puts the *deadline outside it* -- run it on its own
thread and collect the answer through an `mpsc::recv_timeout` -- which is the idiom
`a_blocked_receiver_is_woken_when_the_last_sender_drops` already used a few hundred lines away. Applied to
all three sites (`session/tests.rs`, `watcher/tests.rs`, `tests/consumer_test_surface.rs`), the mutant is
**caught in ~11s** instead of costing a 67s kill.

The general rule: **when a test asserts that a blocking call returns, bounding it by making the call
non-blocking tests something weaker.** Bound the wait *around* it instead.

(Also caught by this: a first injection run reported SURVIVED because the test filter was the package name
rather than a test-name substring, so no tests ran at all. A filter that matches nothing looks exactly
like a mutant that nothing catches.)
**The final sweep, and where the floor is.** `queue.rs` across the three items:

| | after M15.6 | after M15.7 | after M15.11 |
|---|---|---|---|
| caught | 78 | 89 | **95** |
| missed | 8 | 0 | **0** |
| timeout | 14 | 8 | **2** |
| wall clock | 20 min | 14 min | **11 min** |

The two that remain (`Receiver::recv_timeout -> None` and `take -> None`) had **44 and 70 tests already
failed** before the kill, and neither is held by any single slow test. They are the inherent floor rather
than a defect: a mutant that breaks the queue's core makes most of the suite fail, and a suite of failures
takes longer than a suite of passes -- so it overruns a deadline set at 3x the *passing* baseline. No
test-side bound can reach that, and nothing is being missed.

## Moved 2026-09-01 -- M16.1: close the monitor mutation-testing gaps

### <a id="m161"></a>M16.1 -- Adjudicate every mutant missed by the 2026-09-01 `monitor.rs` sweep. *(completed 2026-09-01 22:39:25 -04:00)*

The input sweep ran 317 default-feature tests and reported 15 missed mutants, 18 unviable mutants, and no
timeouts. All 15 misses were genuine observability gaps, grouped into seven contracts rather than treated as
unrelated source edits:

- `Request` debug output names its variant and fields.
- registration, transient fault detail, and permanent stop reason are observable through `Monitor`.
- re-keying updates each routed subscription's stored `DirectoryId`, so later cancellation retires the
  shared watcher.
- a file target's parent-open failure retains retryable versus permanent classification.
- a standing slot reserved only for volume confirmation does not make open-fault recovery interactive.
- `Established` is suppressed when a new route coalesces onto a faulted watcher with no settled tier.
- stopping the last route after a volume-change question removes both subscription and directory resident
  state.

The two duplicated boolean rules were made derived facts: `awaits_open_answer` is shared by first-attempt and
later open retries, and `should_report_established` owns the liveness/fault predicate. Exhaustive truth-table
tests cover both predicates, while behavioral tests prove each is bound to the monitor path rather than tested
cosmetically. Three test-only `DirectoryWatcher` seams put an existing watcher into the transient-fault,
permanent-stop, and volume-change states that the monitor projects or resolves; production behavior is
unchanged.

The all-feature confirming sweep tested 67 mutants in 10 minutes: 47 caught, 18 unviable, **0 missed**, and 2
timeouts. Both timeouts are detections rather than gaps: `Core::submit -> Ok(())` produced 34 failing tests
before the 55-second kill, and `service -> ()` produced 33. Default and all-feature crate tests (including
doctests), default and all-feature crate Clippy, workspace Clippy, and debug/release workspace checks all
passed.
