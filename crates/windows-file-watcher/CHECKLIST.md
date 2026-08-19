# Checklist: windows-file-watcher

Memory-safe Windows path-change watcher. Design and decisions (D-1…D-20) are recorded in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md);
Tier-1 decisions are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md), extended as later milestones complete.

Work items are dependency-ordered. Each milestone ends with integration tests. The implicit
end-of-milestone gate (default **and** `--all-features` build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M2 — Detailed single-directory watcher

- [ ] **M2.1** — Owned directory handle: `CreateFileW(FILE_LIST_DIRECTORY, FILE_SHARE_READ|WRITE|DELETE,
  OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)`; classify open errors (retryable vs
  not-found vs unsupported).

- [ ] **M2.2** — Arm and complete: issue `ReadDirectoryChangesW` through `windows-threadpool-sys`
  `ThreadpoolIo` (the overlapped seam with the generation-stamped identity, D-3/D-4); decode the completion
  into a batch (M1) and re-arm around processing to minimise the inherent loss window.

- [ ] **M2.3** — Deliver batches into a crate-owned queue endpoint (the interim, entirely in-crate delivery
  target for this milestone; the session/receiver split lands in M3, D-11) so no client code runs on the pool
  thread at any milestone; tag records with a `WatchId`; emit `Desync { Overflow }` on a zero-byte completion.

- [ ] **M2.4** — Teardown: cancel the outstanding read, drain the pool I/O, and free the context via
  owned-object `Drop` (D-20), with re-arm suppression inherited from `ThreadpoolIo` rundown.

- [ ] **M2.5** — Integration: create/modify/delete/rename in a temp directory and assert raw actions and
  relative names; force a burst overflow and assert `Desync { Overflow }`; assert clean teardown with an
  operation outstanding.

## M3 — Monitor, session, request queue, watch handle

- [ ] **M3.1** — `Monitor`: owns the servicing path; the request queue is drained by a `ThreadpoolWork`
  that serialises resident-state mutations (D-2); `Monitor::Drop` blocks on full rundown (D-20).

- [ ] **M3.2** — `Session` obtained from the monitor: bundles a request-submission handle (MPSC producers)
  and the crate-owned notification sender (D-2/D-11); provide `monitor.session()` returning the session plus
  the client-side receiver, and a variant accepting a caller-supplied bound.

- [ ] **M3.3** — Finalise the notification queue (D-11): a crate-owned, `Send + Sync`, multi-producer
  bounded sender whose enqueue is non-blocking and infallible and emits `Desync { QueueFull }` on overflow
  (D-12), paired with the client-side receiver the session hands back. No client code runs on a pool thread.

- [ ] **M3.4** — Affine `Watch` (D-5): `#[must_use]`, `Drop` enqueues cancellation, explicit `cancel()`,
  and a `Copy` `WatchId`; subscribe/unsubscribe requests plumbed through the serialised request queue.

- [ ] **M3.5** — Integration: several subscriptions through one session delivering to one receiver; cancel via
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
  per-error-kind overrides), a monitor default overridable per subscription and reduced to a coalesced
  directory watcher's effective policy by the deterministic soonest-recovering rule (min of each field
  across the directory's subscriptions, D-6), mutated only through serialised request-queue items and
  scheduled with `ThreadpoolTimer` — no reactive callback, race-free.

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

- [ ] **M7.3** — Finalise Tier-1 [DESIGN-NOTES.md](DESIGN-NOTES.md) / Tier-2 [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) from the session, with
  every shipped decision cross-referenced.

- [ ] **M7.4** — Opt-in, env-gated stress suite: change churn, fault storms (repeated delete/recreate),
  teardown races, and coalesced multi-subscription load.

- [ ] **M7.5** — Publication readiness: crate metadata, changelog, and a final review pass over the public
  surface for the v1 scope (D-18) and the deferred seams (D-19).

## M∞ — Horizon (ungated, post-v1)

Parked, not pending: these items are gated on nothing and belong to no numbered milestone. They are the
deferred seams of [DESIGN-NOTES.md](DESIGN-NOTES.md) → D-19, and each graduates to a numbered milestone
post-v1 if and when it is chosen. None is an open obligation of any current milestone.

- [ ] **M∞.1** — `ReadDirectoryChangesExW` extended records (`FILE_NOTIFY_EXTENDED_INFORMATION`): surface the
  richer per-record fields on OS versions that support it, behind capability detection, without disturbing
  the basic `FILE_NOTIFY_INFORMATION` surface (D-18/D-19).

- [ ] **M∞.2** — Digest-based change *verification*: an optional mode that confirms a reported change by
  hashing content, trading cost for fewer spurious notifications (D-19).

- [ ] **M∞.3** — Per-volume capability cache: remember detailed-vs-coarse (and extended-record) support per
  volume so establish/re-establish need not re-probe each time (D-17/D-19).
