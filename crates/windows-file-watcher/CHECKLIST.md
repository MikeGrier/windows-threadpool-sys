# Checklist: windows-file-watcher

Memory-safe Windows path-change watcher. Design and decisions (D-1…D-20) are recorded in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md);
Tier-1 decisions land in `DESIGN-NOTES.md` as milestones complete.

Work items are dependency-ordered. Each milestone ends with integration tests. The implicit
end-of-milestone gate (default **and** `--all-features` build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

## M1 — Crate scaffold and notification decode

- [ ] **M1.1** — Scaffold `crates/windows-file-watcher`: `Cargo.toml` with a `cfg(windows)`-gated `lib`,
  path+version dependencies on `windows-overlapped-io-sys` and `windows-threadpool-sys`, and `windows-sys`
  with the needed feature groups; `src/lib.rs` crate-doc skeleton; add the crate to the workspace members.
  Everything is `cfg(windows)`, so the crate resolves to an empty crate elsewhere.

- [ ] **M1.2** — Seed Tier-1 [`DESIGN-NOTES.md`](DESIGN-NOTES.md) and Tier-2 `DESIGN-RATIONALE.md` from
  [the design session](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md) (D-1…D-20), and
  wire the crate into CI so the default and `--all-features` configurations both build, test, and lint.

- [ ] **M1.3** — `FILE_NOTIFY_INFORMATION` record-walk decoder: follow the `NextEntryOffset` chain and
  extract `Action` plus the UTF-16 `FileName` (`FileNameLength` bytes, not NUL-terminated) into a lossless
  relative-name type exposing both `OsString`/`Path` (WTF-8) and raw `&[u16]` (D-8). Malformed offsets are
  handled without out-of-bounds reads.

- [ ] **M1.4** — Change-record surface: map raw `FILE_ACTION_*` to a `ChangeKind` that keeps
  `RenamedOldName` / `RenamedNewName` distinct (D-9); a batch type; and recognition of the zero-byte
  completion as overflow → `Desync { Overflow }` at the decode boundary (D-12).

- [ ] **M1.5** — Tests: ≥10 normal decode cases plus edge cases (empty buffer, single record, multi-record
  chains, maximum-length names, unpaired surrogates, `> MAX_PATH`, zero/truncated buffer → overflow,
  malformed `NextEntryOffset`). Integration: decode a buffer produced by a real blocking
  `ReadDirectoryChangesW` on a temp-directory mutation.

## M2 — Detailed single-directory watcher

- [ ] **M2.1** — Owned directory handle: `CreateFileW(FILE_LIST_DIRECTORY, FILE_SHARE_READ|WRITE|DELETE,
  OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)`; classify open errors (retryable vs
  not-found vs unsupported).

- [ ] **M2.2** — Arm and complete: issue `ReadDirectoryChangesW` through `windows-threadpool-sys`
  `ThreadpoolIo` (the overlapped seam with the generation-stamped identity, D-3/D-4); decode the completion
  into a batch (M1) and re-arm around processing to minimise the inherent loss window.

- [ ] **M2.3** — Deliver batches through a `NotificationSink` (interim direct sink for this milestone);
  tag records with a `WatchId`; emit `Desync { Overflow }` on a zero-byte completion.

- [ ] **M2.4** — Teardown: cancel the outstanding read, drain the pool I/O, and free the context via
  owned-object `Drop` (D-20), with re-arm suppression inherited from `ThreadpoolIo` rundown.

- [ ] **M2.5** — Integration: create/modify/delete/rename in a temp directory and assert raw actions and
  relative names; force a burst overflow and assert `Desync { Overflow }`; assert clean teardown with an
  operation outstanding.

## M3 — Monitor, session, request queue, watch handle

- [ ] **M3.1** — `Monitor`: owns the servicing path; the request queue is drained by a `ThreadpoolWork`
  that serialises resident-state mutations (D-2); `Monitor::Drop` blocks on full rundown (D-20).

- [ ] **M3.2** — `Session` obtained from the monitor: bundles a request-submission handle (MPSC producers)
  and the notification sink (D-2); provide `monitor.session(sink)` and a default-sink variant.

- [ ] **M3.3** — Finalise `NotificationSink`: `Send + Sync`, non-blocking, infallible `deliver`; ship a
  bounded default sink that emits `Desync { QueueFull }` on overflow (D-11/D-12).

- [ ] **M3.4** — Affine `Watch` (D-5): `#[must_use]`, `Drop` enqueues cancellation, explicit `cancel()`,
  and a `Copy` `WatchId`; subscribe/unsubscribe requests plumbed through the serialised request queue.

- [ ] **M3.5** — Integration: several subscriptions through one session delivering to one sink; cancel via
  `Drop` and via `cancel()`; assert no delivery after cancellation completes and in-order delivery within a
  subscription.

## M4 — Coalescing by directory and file targets

- [ ] **M4.1** — Coalesce watchers by directory (D-6): union the `FILE_NOTIFY_CHANGE_*` filters and take the
  maximum subtree flag across a directory's subscriptions; issue one read per directory.

- [ ] **M4.2** — De-multiplex on decode: route each record to the subset of subscriptions whose target and
  filter match (per-subscription filtering, D-6).

- [ ] **M4.3** — File (path) targets (D-7): watch the parent directory non-recursively and filter the leaf
  name; directory targets optionally recursive.

- [ ] **M4.4** — Add/remove a subscription to/from an existing coalesced directory watcher without
  disturbing the others' cadence (re-issue with the updated union only when it actually changes).

- [ ] **M4.5** — Integration: several file-watches plus a recursive directory watch within one tree; assert
  each subscription receives exactly its matching events and nothing else.

## M5 — Fault model and resident retry policy

- [ ] **M5.1** — Establish/re-establish state machine (D-14/D-15): `Opening → ArmingDetailed →
  WatchingDetailed` plus `Cancelling/Closed`; classify every error into reopen-retry, rearm-retry, or (M6)
  downgrade; no terminal state.

- [ ] **M5.2** — Resident retry-policy data (D-16): a backoff value (initial/multiplier/cap/jitter and
  per-error-kind overrides), a monitor default overridable per subscription, mutated only through serialised
  request-queue items and scheduled with `ThreadpoolTimer` — no reactive callback, race-free.

- [ ] **M5.3** — Recovery notifications: `Desync { Reestablished }` for the post-outage gap, and the opt-in
  `Suspended` / `Resumed` brackets (D-13).

- [ ] **M5.4** — Cancellation from any intermediate state — establishing, backing off, or faulted (D-14) —
  quiescing timers and any outstanding operation without racing a re-arm.

- [ ] **M5.5** — Integration: delete then recreate the watched directory; assert the `Suspended` … `Resumed`
  bracket with `Desync { Reestablished }` and that watching resumes; assert cancellation while faulted; verify
  recovery never wedges.

## M6 — Coarse fallback

- [ ] **M6.1** — Coarse handle: `FindFirstChangeNotification`; an owned wrapper whose `Drop` calls
  `FindCloseChangeNotification` (not `CloseHandle`), reaching `ThreadpoolWait` via
  `WaitableHandle::assume_waitable` (D-17).

- [ ] **M6.2** — Coarse watcher: `ThreadpoolWait` per activation → emit `Desync { Coarse }` to the
  directory's subscriptions → `FindNextChangeNotification` re-arm, under the same fault/backoff discipline
  (D-15/D-17).

- [ ] **M6.3** — Downgrade edge in establish (D-17): an unsupported-class error (`ERROR_INVALID_FUNCTION` /
  `ERROR_NOT_SUPPORTED`) transitions to coarse establishment; the mode is re-resolved on each
  establish/re-establish; retryable errors still use the reopen loop.

- [ ] **M6.4** — `Established { mode }` opt-in report (D-13), plus a test seam to force coarse mode
  regardless of the underlying volume.

- [ ] **M6.5** — Integration: force coarse via the seam → assert `Established { Coarse }` and that mutations
  surface as `Desync { Coarse }`; assert coarse teardown closes the notification handle correctly.

## M7 — Documentation, examples, stress

- [ ] **M7.1** — Crate `README.md` and `lib.rs` top-level docs: the monitor/session/watch model, the
  fidelity-and-limitation contract, and the `Desync` primitive.

- [ ] **M7.2** — Runnable examples: a minimal directory watch, a single-file watch, and a fault-recovery
  demonstration.

- [ ] **M7.3** — Finalise Tier-1 `DESIGN-NOTES.md` / Tier-2 `DESIGN-RATIONALE.md` from the session, with
  every shipped decision cross-referenced.

- [ ] **M7.4** — Opt-in, env-gated stress suite: change churn, fault storms (repeated delete/recreate),
  teardown races, and coalesced multi-subscription load.

- [ ] **M7.5** — Publication readiness: crate metadata, changelog, and a final review pass over the public
  surface for the v1 scope (D-18) and the deferred seams (D-19).
