# Checklist: windows-ioring-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-22-ioring-architecture.md](design-sessions/DESIGN-SESSION-2026-08-22-ioring-architecture.md).
M1 through M7 (ring lifecycle through the `ring-copy` sample) are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-22----m1-through-m6-ring-lifecycle-through-consumer-documentation)
and
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-23----m7-ring-copy-a-topology-aligned-sample).

## M6+ -- Model B: explicit-thread delivery and affinity

Parked, not pending. Deferred by the engineer's explicit direction during the 2026-08-22 design session,
with the plan scoped now so the shape is not lost. This is **not** a fallback for a missing capability
(D-3) -- it is the high-performance architecture, and M4's thread-pool path is the convenient one.

- [ ] **M6+.1** -- `DeliveryMode::{ThreadpoolWait, PinnedThread}` as an explicit consumer choice, never an
  automatic degradation.

- [ ] **M6+.2** -- Resolve the contention between a thread parked in `SubmitIoRing(ring, n, INFINITE, ..)`
  and callers wanting to build SQEs. This is the hard part and the reason this is its own milestone: it
  directly contradicts M3.1's `&mut`-enforced serialization, and needs either a submit-ownership handoff
  or an internal lock. Neither is obviously right.

- [ ] **M6+.3** -- Shutdown: waking a thread parked on `INFINITE`. `IORING_OP_NOP` is supported and is the
  wake mechanism.

- [ ] **M6+.4** -- Affinity: binding a ring's thread with `SetThreadGroupAffinity`, and documenting the
  execution-domain pattern (one pinned thread, its ring, its node-local registered pool, its shard).

- [ ] **M6+.5** -- A test seam forcing the pinned-thread path even where the completion event is available,
  so it stays testable on every machine rather than only on hardware that lacks the feature.

- [ ] **M6+.6** -- Decide `IoBuf`: extract to a shared crate, re-export from
  `windows-overlapped-io-sys`, or leave duplicated (D-1). The merge-or-delete decision that duplicate-then-decide
  defers to the point where the new path is proven -- which is here, not earlier.

## M8 -- `FileRef::Raw(HANDLE)` lifetime safety (PR #20 review finding)

A caller can close or reuse the raw `HANDLE` passed to `Batch::read`/`write`/`flush`/`cancel`/
`register_files` before the kernel finishes with it: unlike a buffer (owned by a `Token` until claimed),
`FileRef::Raw` carries no lifetime and nothing in this crate borrow-checks a handle across the async gap
between push and completion.

- [x] **M8.1** -- Ownership model decided: `Batch::read`/`write`/`flush`/`cancel`/`register_files` become
  `unsafe fn` when addressing a raw `HANDLE` (their existing `SAFETY` comments already state the
  caller-keeps-it-alive obligation informally; this makes it a real, compiler-checked boundary), paired
  with a safe `SharedFile` wrapper (`Arc<OwnedHandle>`) for the common case: each push clones the `Arc` into
  the same `Token` that already tracks the operation's buffer (or, for `flush`/`cancel`, a standalone
  `Token<Arc<OwnedHandle>>`), so the underlying handle survives until every operation referencing it is
  claimed or leaked, regardless of what the caller does with its own `SharedFile` clone. Rejected:
  borrowing `FileRef::Raw` for the `Batch`'s own lifetime (does not solve the completion-outlives-the-push-
  call problem) and forcing every raw handle through an owning wrapper the way
  `windows-overlapped-io-sys`'s endpoints do (defeats `FileRef::Raw`'s zero-setup reason to exist).
  `FileRef::Registered` needs none of this and stays unaffected.

- [x] **M8.2** -- Added `SharedFile` (`pub struct SharedFile(Arc<OwnedHandle>)`) with a constructor from
  `OwnedHandle`, `Clone`, and a raw-handle accessor for building an `IORING_HANDLE_REF`.

- [x] **M8.3** -- Marked `Batch::read`/`write`/`flush`/`cancel`/`register_files`'s raw-`HANDLE`-taking
  forms `unsafe fn` with a `# Safety` section stating the real obligation (valid handle, correct access
  rights, remains valid until the pushed operation's completion is observed or the ring runs down).
  **Scope correction found during execution:** `read_registered`/`write_registered` take the identical
  `impl Into<FileRef>` shape for their own `file` argument and carry the same hazard, so they were marked
  `unsafe fn` too, with no separate checklist item -- the original enumeration omitted them by oversight,
  not by decision. Committed as `feat(ioring)!`.

- [x] **M8.4** -- Added safe overloads `read_shared`/`write_shared`/`flush_shared`/`cancel_shared` (plus
  `read_registered_shared`/`write_registered_shared`, for the same reason as M8.3's correction) taking
  `&SharedFile`, cloning the `Arc` into the same `Token` (a tuple with the buffer for `read`/`write`-shaped
  pushes, or `Token<SharedFile>` alone for `flush`/`cancel`, which have no buffer of their own) so a caller
  never needs `unsafe` for the common case. `register_files` gets no `_shared` counterpart: a registration's
  handles must stay valid for the ring's remaining life, a lifetime no single push's `Token` can express.

- [x] **M8.5** -- Integration test
  (`dropping_the_callers_own_sharedfile_clone_does_not_close_a_still_outstanding_handle`,
  `tests/submission_lifecycle.rs`): drop the caller's own `SharedFile` clone (its only external reference)
  while a push against it is still outstanding; the read still completes correctly against a live handle,
  proving the `Arc` clone inside the token -- not the caller's copy -- is what kept it open.

- [x] **M8.6** -- Renamed the API so the safe path gets the plain names: the raw/`unsafe` entry points
  became `read_raw`/`write_raw`/`flush_raw`/`cancel_raw`/`read_registered_raw`/`write_registered_raw`, and
  the safe `SharedFile`-taking overloads (previously `*_shared`) took over the plain names
  `read`/`write`/`flush`/`cancel`/`read_registered`/`write_registered` -- since the safe path is the one
  this crate wants to steer callers toward. `register_files` keeps its plain name unsafe, since it has no
  safe counterpart to make way for.

## M9 -- Cross-ring identity and registration-drop safety (PR #20 review findings)

`UserData`, `RegisteredFile`, and `RegisteredBuffers` indices are each meaningful only against the
specific ring that minted them, but nothing enforced that when more than one `IoRing` exists in the same
process.

- [x] **M9.1** -- Added `RingId` (`ring.rs`): a monotonic, process-lifetime-unique `AtomicU64` counter,
  never the ring's own `HANDLE` (which Windows can reuse for the next object after a ring closes). Every
  `IoRing` gets one at construction; every popped `Completion` now carries the id of the ring that
  produced it (D-17).

- [x] **M9.2** -- `Token::claim_if` now requires both the `UserData` identity and the `RingId` to match,
  closing the gap where two different rings' own zero-based `UserData` counters could coincide. Added
  `claim_if_rejects_a_matching_user_data_from_a_different_ring` (`src/token/tests.rs`).

- [x] **M9.3** -- `RegisteredFile`/`RegisteredFiles`/`PendingFileRegistration` and
  `RegisteredBuffers`/`PendingBufferRegistration` now carry the minting ring's `RingId`; `handle_ref`
  (fallible now) and a new `Batch::check_registration_ring` reject a `FileRef::Registered`/
  `RegisteredBuffers` argument from a different ring with `io::ErrorKind::InvalidInput`, checked before any
  `Build*` call runs. Added `a_registered_file_from_a_different_ring_is_rejected` and
  `a_registered_buffers_from_a_different_ring_is_rejected` (`tests/registration.rs`).

- [x] **M9.4** -- `PendingBufferRegistration` now leaks its buffers (via `ManuallyDrop` plus a
  deliberately empty `Drop`) if dropped without a matching `claim_if`, mirroring `Token`/
  `RegisteredBuffers` (D-18) -- previously it dropped `Vec<B>` normally, freeing memory the kernel might
  still reference from an already-queued `BuildIoRingRegisterBuffers` call. Added
  `dropping_an_unclaimed_pending_buffer_registration_leaks_rather_than_frees` (`tests/registration.rs`).

- [x] **M9.5** -- `Batch::do_submit` now marks itself attempted (`self.submitted = true`) *before*
  propagating `SubmitIoRing`'s `HRESULT`, not after: `submit_and_wait` takes `self` by value, so a failed
  call still runs `Drop` once its caller's `self` goes out of scope, and the old ordering left `Drop` free
  to silently retry an already-attempted submit -- which could succeed on the retry without the original
  caller's `Err` ever saying so.

- [x] **M9.6** -- Output-abstraction cleanup in the two examples the review flagged
  (`examples/model_a_delivery.rs`, `examples/l3_domains.rs`): routed through a single writer seam per the
  repository's architectural pre-step, matching `src/bin/run_scenario.rs` (in `windows-file-watcher`) and
  `examples/ring_copy/main.rs`'s existing pattern. `PLANS.md`'s bare `COMPLETED-PLANS.md`/
  `COMPLETED-CHECKLIST.md` references were also made clickable relative links.


## M10 -- Finish the contract audit against the ten specification-gap categories

[DESIGN-NOTES.md](DESIGN-NOTES.md) -> "Specifying this contract" audits this crate against
[the ten categories](../../DESIGN-NOTES.md#specifying-a-delivery-contract) and reaches four of them: D-17's
`RingId` (categories 4/5, handled correctly), `Completion::synthetic`'s test-only gate (category 10, handled
correctly), D-14's registration-index continuity (category 4, recorded as an explicitly unverified
assumption), and the previously-unstated completion-ordering rule. Categories 1, 2, 3, 6, 8, and 9 were
**not examined**, and that is recorded as "not looked at" rather than "does not apply".

- [ ] **M10.1** -- Audit category 3 (state-dependent legality unenumerated) first: capability negotiation
  (D-6) makes the legal op set a per-ring *runtime* property, so which pushes are legal depends on state the
  type system does not carry. Enumerate, per capability state, which `Batch` methods can succeed and what a
  caller may infer from `supports_raw`.

- [ ] **M10.2** -- Audit the remaining categories (1, 2, 6, 8, 9) against the ring/token/registration
  surface, stating each answer including "unspecified, deliberately" where that is honest.

- [ ] **M10.3** -- Resolve or re-record D-14's unverified registration-index continuity assumption. It is a
  cross-message invariant a consumer can silently depend on; either establish it by measurement (the spike's
  precedent) or state plainly on the public API that index continuity is not guaranteed.

## M11 -- The completion event as a ring primitive (external consumer proposal, 2026-08-28)

Prompted by a consumer proposal and the spike that answered it; the exchange is recorded in
[DESIGN-SESSION-2026-08-28-completion-event-multiplexing.md](design-sessions/DESIGN-SESSION-2026-08-28-completion-event-multiplexing.md),
and the decisions are [D-19](DESIGN-NOTES.md#d-19) through [D-22](DESIGN-NOTES.md#d-22).

M11.1 is a correctness fix against shipped 0.1.2 behaviour and is deliberately first and independent:
it must not wait on the API work behind it.

- [ ] **M11.1** -- Fix `EventDelivery`'s stranded-backlog bug ([D-19](DESIGN-NOTES.md#d-19)). A ring handed
  to `EventDelivery::new` with completions already in its CQ never delivers them: the attach does not
  signal (the queue was already non-empty), and nothing afterwards signals either, because the queue never
  returns to empty. Its rustdoc claims the opposite ("including any that were already queued when `ring`
  was handed over"), so the doc is wrong as well as the behaviour. Land the repro from the spike as an
  integration test first (submit, let the completion land, *then* hand the ring over, assert the callback
  runs), then fix by signalling the event once after `wait.arm(..)` -- the drain/re-arm/drain callback body
  is already correct and needs no change. The existing M4 test only ever hands over a fresh ring, which is
  why this passed CI.

- [ ] **M11.2** -- Add `IoRing::completion_event(&mut self) -> io::Result<OwnedHandle>`
  ([D-20](DESIGN-NOTES.md#d-20)): capability-check `IORING_FEATURE_SET_COMPLETION_EVENT`, create and own an
  auto-reset event, `SetIoRingCompletionEvent`, signal it once, and return a duplicate handle. Idempotent --
  repeat calls return another duplicate of the same event rather than attaching a new one. The rustdoc
  states the [D-19](DESIGN-NOTES.md#d-19) contract in full: signalled on empty -> non-empty, drain to empty
  before waiting again, a wake with nothing to pop is normal.

- [ ] **M11.3** -- Tests for the M11.2 contract, written against the *stated* rules rather than against
  current behaviour: the backlog case (submit, let completions land, call `completion_event`, assert the
  returned handle signals), the setup-signal case, the idempotence case, and a multiplexed-wait case that
  waits on the ring's event alongside an unrelated event and asserts the ring still wakes after the
  unrelated one fires. This last one is the configuration that makes an edge-trigger violation observable
  at all, and its absence is why M11.1's bug survived.

- [ ] **M11.4** -- Re-express `EventDelivery::new` on top of `IoRing::completion_event`, removing the
  second `SetIoRingCompletionEvent` call site ([D-20](DESIGN-NOTES.md#d-20)). `ThreadpoolWait` needs an
  owned waitable, which the returned duplicate satisfies. M11.1's regression test must still pass
  unchanged -- it is the check that the consolidation preserved the fix rather than reintroducing the bug
  behind a new seam.

- [ ] **M11.5** -- Gate `windows-threadpool-sys` behind a default-on `threadpool` feature
  ([D-22](DESIGN-NOTES.md#d-22)), with `EventDelivery` and its tests behind the same gate, and extend CI to
  build and test **both** feature combinations. The second combination is the whole point: without it the
  `default-features = false` path rots silently.

- [ ] **M11.6** -- Document the wakeup shapes and both contract gaps across every place that states them --
  `lib.rs`'s "Choosing a delivery architecture", `README.md`, and the "Two delivery architectures" section
  of [DESIGN-NOTES.md](DESIGN-NOTES.md). Two facts to state: that Model B's wakeup source is separable from
  Model B's identity (a caller may own its ring and still wait on it alongside other handles), and that
  `drain_preceding`'s barrier stops at the ring's edge. Per CONTRACT INTEGRITY this is a blast-radius
  sweep, not a single edit: grep `drain_preceding`, "two delivery", and "completion event" across `src/`,
  `tests/`, `examples/`, and `*.md`, and fix or account for every hit.

- [ ] **M11.7** -- An example putting the multiplexed wait together end to end: a caller-owned ring whose
  completion event is waited on via `WaitForMultipleObjects` alongside a shutdown event, draining to empty
  on every pass. This is the shape the requesting consumer is building, and an example is what stops the
  next consumer from rediscovering [D-19](DESIGN-NOTES.md#d-19) the hard way.
