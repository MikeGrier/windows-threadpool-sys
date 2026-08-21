# Checklist: windows-file-watcher

Memory-safe Windows path-change watcher. The design session that opened the crate recorded D-1...D-20 in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md).
The authoritative Tier-1 set is [DESIGN-NOTES.md](DESIGN-NOTES.md), which now runs to **D-31** -- later
decisions (D-21 from M1 review, D-22 from M2.1, D-23/D-24 from M2.2, D-26 from M2.3, and D-25/D-27...D-31
from the [2026-08-21 fault-protocol session](design-sessions/DESIGN-SESSION-2026-08-21-fault-protocol-and-doorbells.md),
which **overturned D-16**) are added there as milestones complete.

Work items are dependency-ordered. Each milestone ends with integration tests. The implicit
end-of-milestone gate (default **and** `--all-features` build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

> **NEXT ACTIONABLE ITEM: M2.1.** M1 is archived; nothing else is in progress. Note that a *satisfied
> cross-component prerequisite does not make its item startable* -- M17 in `windows-threadpool-sys` cleared
> the external dependency for M6.1, but M6.1 remains gated behind M2 through M5 by ordinary intra-component
> dependency order, because a coarse watcher has no subscriptions to notify (M3/M4) and no fault machine to
> re-establish through (M5) until those land. Work the milestones in order.

## M2 -- Detailed single-directory watcher

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

- [ ] **M2.4** -- Teardown: cancel the outstanding read, drain the pool I/O, and free the context via
  owned-object `Drop` (D-20), with re-arm suppression inherited from `ThreadpoolIo` rundown. The arm gate of
  D-23 already provides the suppression; formalise it, and note that the same not-re-arming state is what
  D-29 reuses for backpressure and D-28 for faults.

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
  the next batch (D-12); reject a zero bound at construction. The crate never calls into client code on its
  cadence path.

- [ ] **M3.3.1** -- The CQ doorbell (D-25): `Receiver::doorbell()` returning a manual-reset event handle,
  created lazily so a `recv()`-only client allocates no kernel object, so a client can drain from its own
  `ThreadpoolWait` rather than dedicating a thread to a blocking `recv()`. Crate-owned, not a client trait --
  the receiver resets under the queue lock on observing empty and the sender sets after enqueue, making lost
  wakeups impossible by construction and leaving only harmless spurious ones. Record the rejected trait
  alternative in the module docs, since "why isn't this a trait?" is the obvious question.

- [ ] **M3.3.2** -- The SQ doorbell edge-trigger (D-25): `ThreadpoolWork::submit()` is already the request
  queue's ring, but each call queues another drain, and they do not coalesce -- subscribing to 500 paths
  queues 500 drains, 499 of which find the queue already emptied. Ring only on the empty -> non-empty
  transition, computed under the queue lock.

- [ ] **M3.3.3** -- Request completions (D-30): every request yields a completion carried on the
  notification queue, correlated by `WatchId`, so ordering against data is structural rather than temporal.
  Includes the permanent subscribe failures of D-22 (`NotADirectory`, `InvalidPath`), which have no retry
  path and would otherwise leave a client holding a `Watch` that can never fire and never says so.

- [ ] **M3.3.4** -- Queue-full backpressure (D-29): the monitor stops draining the request queue when the
  notification queue cannot accept a completion, so backpressure lands on the client's own `subscribe()`
  call on the client's own thread. Never block at the enqueue -- the writer may hold a pool thread while the
  client's drain needs one, which is a deadlock rather than backpressure.

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

## M5 -- Fault model and the retry protocol

- [ ] **M5.1** -- Establish/re-establish state machine (D-14/D-15): `Opening -> ArmingDetailed ->
  WatchingDetailed` plus `Cancelling/Closed`; classify every error into reopen-retry, rearm-retry, or (M6)
  downgrade; no terminal state.

- [ ] **M5.2** -- The fault latch (D-28): a fault is watcher state, not a queued item -- one error code plus
  one bit, allocated with the watcher. A fault report is control data generated on the cadence, so it can
  neither be dropped (the watch would silently never recover) nor block (deadlock); latching costs no queue
  slot and cannot fail. A watcher cannot be faulted twice concurrently, because a faulted watcher is not
  running. Generalise the same treatment to every `Desync`, extending what D-11 already does for
  `QueueFull`: reporting that the queue filled must not itself require queue space.

- [ ] **M5.3** -- The retry protocol (D-27, superseding D-16): each subscription selects **defaults** or
  **interactive** at registration. On fault the watcher latches and schedules *nothing*; interactive
  subscriptions receive a control message carrying the `WatchId`, the failing operation (open vs arm), and
  the error code, and answer with the next delay. Because a directory is one coalesced watcher over several
  subscriptions (D-6), ask every subscription and take the **earliest** answer, counting a decliner at its
  default rather than cancelling it, then clamp to the floor. Values from `Azure/m`'s shipped code:
  **500 ms default, 50 ms floor**, with separate open-failure and arm-failure defaults. Scheduling only
  after the answer is what removes D-16's race -- there is no timer to race because none was armed.

- [ ] **M5.4** -- Recovery notifications: `Desync { Reestablished }` for the post-outage gap, and the opt-in
  `Suspended` / `Resumed` brackets (D-13).

- [ ] **M5.5** -- Cancellation from any intermediate state -- establishing, backing off, latched-faulted, or
  awaiting a retry answer (D-14) -- quiescing timers and any outstanding operation without racing a re-arm.

- [ ] **M5.6** -- Observable stall and diagnostics (D-31): a watcher parked not-re-arming (faulted per D-28,
  or backpressured per D-29) is indistinguishable from "nothing is changing" unless reported, so expose the
  state and emit a diagnostic. Settle the transport first: a library emitting output is a dependency
  decision (`eprintln!` is unfilterable and wrong for a library; the `log` facade is near-zero cost when no
  logger is installed but is a public dependency), and per the repository's architectural pre-step rule the
  first emission site must introduce an output abstraction rather than a call. It must **not** be a
  client-supplied sink, which would be a callback on our path (D-2).

- [ ] **M5.7** -- Integration: delete then recreate the watched directory; assert the `Suspended` ...
  `Resumed` bracket with `Desync { Reestablished }` and that watching resumes; assert an interactive
  subscription is asked and its answer honoured, that the earliest of several answers wins, that a decliner
  is counted at its default, and that the floor clamps a zero answer; assert cancellation while faulted and
  while awaiting an answer; verify recovery never wedges.

## M6 -- Coarse fallback

- [ ] **M6.1** -- Coarse handle: `FindFirstChangeNotification`, owned and closed with
  `FindCloseChangeNotification` (not `CloseHandle`), reaching `ThreadpoolWait` through the custom-close
  waitable owner -- a std `OwnedHandle` would be closed with `CloseHandle` by the pool on teardown,
  which is the wrong routine for a change-notification handle. The two-tier arrangement this serves is D-17;
  the ownership mechanism itself is a `windows-threadpool-sys` decision, recorded in the workspace-root
  [DESIGN-NOTES.md](../../DESIGN-NOTES.md) under "A wait target owns its close routine".

  > **CROSS-COMPONENT PREREQUISITE -- SATISFIED 2026-08-21:** component `crates/windows-threadpool-sys` ->
  > M17 (custom-close owner for non-`CloseHandle` wait targets, across both the direct and `CleanupGroup`
  > teardown paths) has landed. See
  > [../windows-threadpool-sys/COMPLETED-CHECKLIST.md](../windows-threadpool-sys/COMPLETED-CHECKLIST.md).
  > The seam to use is `WaitableHandle::assume_waitable_with(raw, FindCloseChangeNotification)`, which works
  > with both `ThreadpoolWait::new` and `CleanupGroup::create_wait`.
  >
  > **This clears only the external dependency.** M6.1 is still gated by M2 through M5 in the ordinary
  > dependency order of this crate and is not startable ahead of them.

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

- [ ] **M8.1** -- Add the [wtf-string](../wtf-string/README.md) dependency (published; pin the current
  release) and migrate `RelativeName` from its hand-rolled `Box<[u16]>` to `Wtf16Str` / `Wtf16String`, so
  decoded names carry the native-`u16`, conversion-free representation and feed Windows APIs without
  re-encoding. Preserve the lossless `OsString`/`Path` and raw-`&[u16]` surface (D-8).

  > **CROSS-COMPONENT PREREQUISITE -- SATISFIED:** component `crates/wtf-string` -> M5 (Windows
  > `OsStr`/`OsString` interop) is complete, as is the crate's whole v1 plan (M1 through M10), archived in
  > [../wtf-string/COMPLETED-CHECKLIST.md](../wtf-string/COMPLETED-CHECKLIST.md). As with M6.1 above, this
  > clears only the external dependency; M8.1 still follows M2 through M7 in order.

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
