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
