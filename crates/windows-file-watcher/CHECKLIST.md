# Checklist: windows-file-watcher

Memory-safe Windows path-change watcher. Design and decisions (D-1...D-20) are recorded in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md);
Tier-1 decisions are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md), extended as later milestones complete.

Work items are dependency-ordered. Each milestone ends with integration tests. The implicit
end-of-milestone gate (default **and** `--all-features` build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M2 -- Detailed single-directory watcher

- [ ] **M2.1** -- Owned directory handle: `CreateFileW(FILE_LIST_DIRECTORY, FILE_SHARE_READ|WRITE|DELETE,
  OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)`; classify open errors (retryable vs
  not-found vs unsupported).

- [ ] **M2.2** -- Arm and complete: issue `ReadDirectoryChangesW` through `windows-threadpool-sys`
  `ThreadpoolIo` (the overlapped seam with the generation-stamped identity, D-3/D-4); decode the completion
  into a batch (M1) and re-arm around processing to minimise the inherent loss window.

- [ ] **M2.3** -- Deliver batches into a crate-owned queue endpoint (the interim, entirely in-crate delivery
  target for this milestone; the session/receiver split lands in M3, D-11) so no client code runs on the pool
  thread at any milestone; tag records with a `WatchId`; emit `Desync { Overflow }` on a zero-byte completion.

- [ ] **M2.4** -- Teardown: cancel the outstanding read, drain the pool I/O, and free the context via
  owned-object `Drop` (D-20), with re-arm suppression inherited from `ThreadpoolIo` rundown.

- [ ] **M2.5** -- Integration: create/modify/delete/rename in a temp directory and assert raw actions and
  relative names; force a burst overflow and assert `Desync { Overflow }`; assert clean teardown with an
  operation outstanding.

## M3 -- Monitor, session, request queue, watch handle

- [ ] **M3.1** -- `Monitor`: owns the servicing path; the request queue is drained by a `ThreadpoolWork`
  that serialises resident-state mutations (D-2); `Monitor::Drop` blocks on full rundown (D-20).

- [ ] **M3.2** -- `Session` obtained from the monitor: bundles a request-submission handle (MPSC producers)
  and the crate-owned notification sender (D-2/D-11); provide `monitor.session()` returning the session plus
  the client-side receiver, and a variant accepting a caller-supplied bound.

- [ ] **M3.3** -- Finalise the notification queue (D-11): a crate-owned, `Send + Sync`, multi-producer
  bounded sender whose enqueue is non-blocking and infallible, paired with the client-side receiver the
  session hands back. On overflow it drops the batch and latches a per-`WatchId` `Desync { QueueFull }` as
  control state *outside* the bounded queue (coalesced, idempotent), guaranteed to reach the receiver before
  the next batch (D-12); reject a zero bound at construction. No client code runs on a pool thread.

- [ ] **M3.4** -- Affine `Watch` (D-5): `#[must_use]`, `Drop` enqueues cancellation, explicit `cancel()`,
  and a `Copy` `WatchId`; subscribe/unsubscribe requests plumbed through the serialised request queue.

- [ ] **M3.5** -- Integration: several subscriptions through one session delivering to one receiver; cancel via
  `Drop` and via `cancel()`; assert no delivery after cancellation completes and in-order delivery within a
  subscription; saturate the queue and assert the dropped batch surfaces as `Desync { QueueFull }` for each
  affected `WatchId` and that delivery recovers once the receiver drains.

## M4 -- Coalescing by directory and file targets

- [ ] **M4.1** -- Coalesce watchers by directory (D-6): union the `FILE_NOTIFY_CHANGE_*` filters and take the
  maximum subtree flag across a directory's subscriptions; issue one read per directory.

- [ ] **M4.2** -- De-multiplex on decode: route each record to the subset of subscriptions whose target and
  filter match (per-subscription filtering, D-6).

- [ ] **M4.3** -- File (path) targets (D-7): watch the parent directory non-recursively and filter the leaf
  name; directory targets optionally recursive.

- [ ] **M4.4** -- Add/remove a subscription to/from an existing coalesced directory watcher without
  disturbing the others' cadence (re-issue with the updated union only when it actually changes).

- [ ] **M4.5** -- Integration: several file-watches plus a recursive directory watch within one tree; assert
  each subscription receives exactly its matching events and nothing else.

## M5 -- Fault model and resident retry policy

- [ ] **M5.1** -- Establish/re-establish state machine (D-14/D-15): `Opening -> ArmingDetailed ->
  WatchingDetailed` plus `Cancelling/Closed`; classify every error into reopen-retry, rearm-retry, or (M6)
  downgrade; no terminal state.

- [ ] **M5.2** -- Resident retry-policy data (D-16): a backoff value (initial/multiplier/cap/jitter and
  per-error-kind overrides), a monitor default overridable per subscription and reduced to a coalesced
  directory watcher's effective policy by the deterministic soonest-recovering rule (min of each field
  across the directory's subscriptions, D-6), mutated only through serialised request-queue items and
  scheduled with `ThreadpoolTimer` -- no reactive callback, race-free.

- [ ] **M5.3** -- Recovery notifications: `Desync { Reestablished }` for the post-outage gap, and the opt-in
  `Suspended` / `Resumed` brackets (D-13).

- [ ] **M5.4** -- Cancellation from any intermediate state -- establishing, backing off, or faulted (D-14) --
  quiescing timers and any outstanding operation without racing a re-arm.

- [ ] **M5.5** -- Integration: delete then recreate the watched directory; assert the `Suspended` ... `Resumed`
  bracket with `Desync { Reestablished }` and that watching resumes; assert cancellation while faulted; verify
  recovery never wedges.

## M6 -- Coarse fallback

- [ ] **M6.1** -- Coarse handle: `FindFirstChangeNotification`, owned and closed with
  `FindCloseChangeNotification` (not `CloseHandle`), reaching `ThreadpoolWait` through the custom-close
  waitable owner (D-17) -- a std `OwnedHandle` would be closed with `CloseHandle` by the pool on teardown,
  which is the wrong routine for a change-notification handle.

  > **CROSS-COMPONENT PREREQUISITE:** requires component `crates/windows-threadpool-sys` -> M17
  > (custom-close owner for non-`CloseHandle` wait targets, across both the direct and `CleanupGroup`
  > teardown paths) to land first. See
  > [../windows-threadpool-sys/CHECKLIST.md](../windows-threadpool-sys/CHECKLIST.md).

- [ ] **M6.2** -- Coarse watcher: `ThreadpoolWait` per activation -> emit `Desync { Coarse }` to the
  directory's subscriptions -> `FindNextChangeNotification` re-arm, under the same fault/backoff discipline
  (D-15/D-17).

- [ ] **M6.3** -- Downgrade edge in establish (D-17): an unsupported-class error (`ERROR_INVALID_FUNCTION` /
  `ERROR_NOT_SUPPORTED`) transitions to coarse establishment; the mode is re-resolved on each
  establish/re-establish; retryable errors still use the reopen loop.

- [ ] **M6.4** -- `Established { mode }` opt-in report (D-13), plus a test seam to force coarse mode
  regardless of the underlying volume.

- [ ] **M6.5** -- Integration: force coarse via the seam -> assert `Established { Coarse }` and that mutations
  surface as `Desync { Coarse }`; assert coarse teardown closes the notification handle correctly.

## M7 -- Documentation, examples, stress

- [ ] **M7.1** -- A crate README and the [lib.rs](src/lib.rs) top-level docs: the monitor/session/watch model, the
  fidelity-and-limitation contract, and the `Desync` primitive.

- [ ] **M7.2** -- Runnable examples: a minimal directory watch, a single-file watch, and a fault-recovery
  demonstration.

- [ ] **M7.3** -- Finalise Tier-1 [DESIGN-NOTES.md](DESIGN-NOTES.md) / Tier-2 [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) from the session, with
  every shipped decision cross-referenced.

- [ ] **M7.4** -- Opt-in, env-gated stress suite: change churn, fault storms (repeated delete/recreate),
  teardown races, and coalesced multi-subscription load.

- [ ] **M7.5** -- Publication readiness: crate metadata, changelog, and a final review pass over the public
  surface for the v1 scope (D-18) and the deferred seams (D-19).

## M8 -- Adopt wtf-string for relative names

- [ ] **M8.1** -- Migrate `RelativeName` from its hand-rolled `Box<[u16]>` to [wtf-string](../wtf-string/README.md)'s
  `Wtf16Str` / `Wtf16String`, so decoded names carry the native-`u16`, conversion-free representation and feed
  Windows APIs without re-encoding. Preserve the lossless `OsString`/`Path` and raw-`&[u16]` surface (D-8).

  > **CROSS-COMPONENT PREREQUISITE:** requires component `crates/wtf-string` -> M5 (Windows `OsStr`/`OsString`
  > interop) to land first. See [../wtf-string/CHECKLIST.md](../wtf-string/CHECKLIST.md).

- [ ] **M8.2** -- Integration test: after adoption, decode a real completion buffer and assert the relative
  name's raw `&[u16]` units, its lossless `OsString`/`Path` conversion (including an unpaired surrogate), and
  a direct wide-pointer (`as_ptr()`) hand-off to a Windows API, verifying the representation change preserves
  the public lossless-conversion contract (D-8).

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
