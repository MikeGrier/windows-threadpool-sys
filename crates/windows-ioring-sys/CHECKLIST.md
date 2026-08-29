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
[the ten categories](../../DESIGN-NOTES.md#specifying-a-delivery-contract). The first pass reached four of
them: D-17's `RingId` (categories 4/5, handled correctly), `Completion::synthetic`'s test-only gate
(category 10, handled correctly), D-14's registration-index continuity (category 4, recorded as an explicitly
unverified assumption), and the previously-unstated completion-ordering rule. Categories 1, 2, 3, 6, 8, and 9
were **not examined**, recorded as "not looked at" rather than "does not apply".

M10.1-M10.3 closed that out, so all ten are now examined. The audit also turned up two API gaps rather than
mere statement gaps; they are M10.4 and M10.5 below rather than notes in the design record, per "design notes
are not a work queue".

- [x] **M10.1** -- Audited category 3 (state-dependent legality unenumerated) against capability negotiation
  ([D-28](DESIGN-NOTES.md#d-28)): [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "Category 3" now tabulates which
  `Batch` methods each probed op gates, and states the four rules the prose left silent -- `supports` answers
  for the kernel's op table rather than this crate's push surface (`Op::Nop` gates no `Batch` method at all,
  which is exactly the op M6+.3's shutdown case reaches for); `supports_raw` accepts named codes too and
  differs from `supports` in caching rather than truth, while never widening what `Batch` can push; every
  legality check runs before `reserve_user_data`, so a rejected push strands no identity; and the registration
  one-shot is enforced against the registered count, making the real rule "at most one registration that
  assigned an index" and making a *failed* registration unretryable on that ring. Corrected the `supports`,
  `supports_raw`, `Op`, `register_files`, and `register_buffers` rustdoc that stated these wrongly or not at
  all, and added an integration test binding the zero-length-registration rule.

- [x] **M10.2** -- Audited the remaining categories (1, 2, 6, 8, 9) against the ring/token/registration
  surface; [DESIGN-NOTES.md](DESIGN-NOTES.md) carries one section each. Category 1: file addressing and
  buffer addressing are genuinely orthogonal (all four combinations reachable), but file addressing and
  *safety* co-vary and in the wrong direction -- `FileRef::Registered` is reachable only through an
  `unsafe fn` whose safety obligation is vacuous for that very input
  ([D-29](DESIGN-NOTES.md#d-29), queued as M10.4). Category 2: **every successfully queued SQE produces
  exactly one completion**, unconditionally -- the rule `run_down`'s termination silently depended on --
  plus the two "may" readings that were too weak (`submit*` returns entries submitted, not completed; a
  cancel is a request that yields a second completion rather than replacing its target's). Category 6: a
  completion matching no live `Token` is normal, via four distinct routes. Category 8: this crate joins
  nothing, deliberately, and now says so. Category 9: `io::Error::kind()` is lossy exactly where the
  `HRESULT` is not, discriminating this crate's own rejections and never the kernel's
  ([D-30](DESIGN-NOTES.md#d-30), predicate queued as M10.5). Category 7 falls out of 2 and 6 jointly, so
  all ten categories are now examined.

- [x] **M10.3** -- Resolved D-14's unverified registration-index continuity assumption, by **dissolution
  rather than measurement** ([D-31](DESIGN-NOTES.md#d-31)). D-14 justified the eager advance on the grounds
  that erring early "can only ever waste indices, never collide two registrations onto the same index" -- and
  that collision needs a *second* registration, which the PR #20 review response forbade the day after D-14
  was written. So `base_index` is always zero, no later base index is ever derived from the count, and the
  kernel's claim timing has no observable consequence; measuring it would settle a fact with nothing
  downstream of it. Took the checklist's second branch for the residue that *is* observable: the public
  counts now state on themselves that they report a **reserved** count, not a confirmed one -- already
  advanced before any completion is popped, and still advanced after a registration whose completion failed
  (which is why that registration cannot be retried, [D-28](DESIGN-NOTES.md#d-28)). Added a test binding
  reserved-at-queue-time, gave D-14 the required adjacent status marker, and corrected the audit section's
  category-4 paragraph, which still described the assumption as live.

- [ ] **M10.4** -- Give `FileRef::Registered` safe entry points ([D-29](DESIGN-NOTES.md#d-29)). Today every
  safe push hardcodes `FileRef::Raw` from its `SharedFile`, so using a registered file forces an `unsafe`
  call whose contract is vacuous for that input -- the index is minted by this crate, checked against the
  minting ring (D-17), and names a table the ring owns, leaving the caller nothing to keep alive. Vacuous
  `unsafe` is worse than none: it trains a caller to discharge safety contracts by rote. Accept a
  `RegisteredFile` without `unsafe` across the read/write/flush/cancel surface, including the
  registered-buffer variants, and cover the file-registered x buffer-registered combination the safe API
  currently cannot express at all.

- [ ] **M10.5** -- A named predicate for the ring conditions a consumer must branch on, starting with
  `IORING_E_SUBMISSION_QUEUE_FULL` ([D-30](DESIGN-NOTES.md#d-30)). Every push's rustdoc names queue-full as
  the backpressure signal, and the only way to detect it is a `downcast_ref::<IoRingError>()` plus an
  `HRESULT` comparison -- documented on `IoRingError` as of M10.2, but still hand-rolled at every call site
  that needs it. Do **not** map `IORING_E_*` onto `io::ErrorKind`: D-30 refuses that as trading an honest
  `Other` for a lossy guess.

## M11 -- The completion event as a ring primitive (external consumer proposal, 2026-08-28)

Prompted by a consumer proposal and the spike that answered it; the exchange is recorded in
[DESIGN-SESSION-2026-08-28-completion-event-multiplexing.md](design-sessions/DESIGN-SESSION-2026-08-28-completion-event-multiplexing.md),
and the decisions are [D-19](DESIGN-NOTES.md#d-19) through [D-22](DESIGN-NOTES.md#d-22).

M11.1 and M11.2 build the primitive; M11.3 then consolidates `EventDelivery` onto it *and* fixes the
stranded-backlog bug in one change. Those were separate items when the fix looked like it needed to ship
ahead of the API work; they are merged because in practice the two land minutes apart, and splitting them
would mean writing a `SetEvent`-after-arm patch that M11.3 immediately deletes.

- [ ] **M11.1** -- Add `IoRing::completion_event(&mut self) -> io::Result<OwnedHandle>`
  ([D-20](DESIGN-NOTES.md#d-20)): capability-check `IORING_FEATURE_SET_COMPLETION_EVENT`, create and own an
  auto-reset event, `SetIoRingCompletionEvent`, signal it once, and return a duplicate handle. Idempotent --
  repeat calls return another duplicate of the same event rather than attaching a new one. The rustdoc
  states the [D-19](DESIGN-NOTES.md#d-19) contract in full: signalled on empty -> non-empty, drain to empty
  before waiting again, a wake with nothing to pop is normal.

- [ ] **M11.2** -- Tests for the M11.1 contract, written against the *stated* rules rather than against
  current behaviour: the backlog case (submit, let completions land, call `completion_event`, assert the
  returned handle signals), the setup-signal case, the idempotence case, and a multiplexed-wait case that
  waits on the ring's event alongside an unrelated event and asserts the ring still wakes after the
  unrelated one fires. This last one is the configuration that makes an edge-trigger violation observable
  at all, and its absence is why the M11.3 bug survived CI.

- [ ] **M11.3** -- Re-express `EventDelivery::new` on top of `IoRing::completion_event`, which removes the
  second `SetIoRingCompletionEvent` call site ([D-20](DESIGN-NOTES.md#d-20)) and fixes the stranded-backlog
  bug ([D-19](DESIGN-NOTES.md#d-19)) in the same change. The bug: a ring handed to `EventDelivery::new`
  with completions already in its CQ never delivers them, because the attach does not signal (the queue
  was already non-empty) and nothing afterwards signals either (the queue never returns to empty). Its
  rustdoc claims the opposite ("including any that were already queued when `ring` was handed over"), so
  the doc is wrong as well as the behaviour, and must be corrected in the same commit. Land the spike's
  repro as a failing integration test first (submit, let the completion land, *then* hand the ring over,
  assert the callback runs), then consolidate: `completion_event`'s signal-once-on-attach is what makes it
  pass, and `ThreadpoolWait` accepts the returned duplicate as its owned waitable. The drain/re-arm/drain
  callback body is already correct and needs no change. The existing M4 test only ever hands over a fresh
  ring, which is why this passed CI.

- [ ] **M11.4** -- Gate `windows-threadpool-sys` behind a default-on `threadpool` feature
  ([D-22](DESIGN-NOTES.md#d-22)), with `EventDelivery` and its tests behind the same gate, and extend CI to
  build and test **both** feature combinations. The second combination is the whole point: without it the
  `default-features = false` path rots silently.

- [ ] **M11.5** -- Document the wakeup shapes and both contract gaps across every place that states them --
  `lib.rs`'s "Choosing a delivery architecture", `README.md`, and the "Two delivery architectures" section
  of [DESIGN-NOTES.md](DESIGN-NOTES.md). Two facts to state: that Model B's wakeup source is separable from
  Model B's identity (a caller may own its ring and still wait on it alongside other handles), and that
  `drain_preceding`'s barrier stops at the ring's edge. Per CONTRACT INTEGRITY this is a blast-radius
  sweep, not a single edit: grep `drain_preceding`, "two delivery", and "completion event" across `src/`,
  `tests/`, `examples/`, and `*.md`, and fix or account for every hit.

- [ ] **M11.6** -- An example putting the multiplexed wait together end to end: a caller-owned ring whose
  completion event is waited on via `WaitForMultipleObjects` alongside a shutdown event, draining to empty
  on every pass. This is the shape the requesting consumer is building, and an example is what stops the
  next consumer from rediscovering [D-19](DESIGN-NOTES.md#d-19) the hard way.

## M12 -- Durability: expose it, and stop defaulting it wrong

The kernel exposes a durability parameter on writes (`FILE_WRITE_FLAGS`) and on flushes
(`FILE_FLUSH_MODE`); this crate hardcodes both, so a consumer sees ordering but no way to express
durability at all. Worse, [D-23](DESIGN-NOTES.md#d-23) measured that an unflagged flush does *not*
cover preceding writes -- which makes `Batch::flush(&file, PushOptions::default())`, the obvious
spelling, a silent data-loss bug rather than a missing feature.

Decisions: [D-23](DESIGN-NOTES.md#d-23) through [D-25](DESIGN-NOTES.md#d-25). Measurements are
reproduced by the drain spike recorded in
[DESIGN-SESSION-2026-08-28-external-consumer-correspondence.md](design-sessions/DESIGN-SESSION-2026-08-28-external-consumer-correspondence.md).

M12.1 is first because it is a correctness defect in shipped 0.1.2, not an enhancement.

- [ ] **M12.1** -- Make the barrier decision explicit for flushes ([D-23](DESIGN-NOTES.md#d-23),
  [D-25](DESIGN-NOTES.md#d-25)). Today `Batch::flush`/`flush_raw` accept `PushOptions`, whose default
  carries no barrier, so the natural call produces a flush that can complete while the writes it is
  meant to cover are still outstanding. Remove the ability to express that by accident: the flush
  entry points take the barrier decision as a required argument (a two-variant type -- "covers
  preceding operations" versus "unordered" -- not a `bool`, so the call site reads correctly), with
  the unordered form documented as almost never what a caller wants. Rustdoc states the measured
  contract and links [D-23](DESIGN-NOTES.md#d-23).

- [ ] **M12.2** -- A test proving the contract rather than the implementation: writes plus an
  *unordered* flush must be observable completing out of order (the spike sees 17-23 of 32), while
  writes plus a *covering* flush must always place the flush last. Needs the spike's conditions --
  `FILE_FLAG_NO_BUFFERING` over a pre-written extent -- because buffered or extending writes complete
  in submission order and make the test vacuous. Guard it so that a machine where the control shows
  no reordering skips rather than falsely passing.

- [ ] **M12.3** -- Expose `FILE_WRITE_FLAGS` on the write entry points
  ([D-25](DESIGN-NOTES.md#d-25)), as a typed option rather than a raw flag word. Rustdoc must state
  what write-through is and is not: a first-level cache directive that shortens a later flush, **not**
  a durability guarantee and **not** FUA -- the conflation that cost this exchange a wrong
  recommendation.

- [ ] **M12.4** -- Expose `FILE_FLUSH_MODE` on the flush entry points
  ([D-25](DESIGN-NOTES.md#d-25)) as a typed enum (`Default`, `Data`, `MinMetadata`, `NoSync`).
  `NoSync` needs the loudest documentation in the crate: it skips the device sync, so it is the one
  mode that does not make anything durable. Note in passing that the existence of `NoSync` as a
  distinct mode is the evidence that the other three do issue the sync.

- [ ] **M12.5** -- Document durability across every place that states it, as a CONTRACT INTEGRITY
  blast-radius sweep rather than a single edit: `lib.rs`, `README.md`, the flush and write rustdoc,
  and the "Durability on the ring" section of [DESIGN-NOTES.md](DESIGN-NOTES.md). Three facts must
  appear wherever durability is discussed -- the ring has no FUA, the flush is the only durability
  primitive, and a flush without the barrier covers nothing. Grep `flush`, `durab`, `write_through`,
  and `drain_preceding` across `src/`, `tests/`, `examples/`, and `*.md`.

## M13 -- Worked example: consumer-side durability (an epoch-committed log)

[D-26](DESIGN-NOTES.md#d-26) puts durability *policy* with the consumer and Windows *mechanism*
here, which leaves a gap: without a demonstration, every consumer rediscovers the same composition,
and the three measured contracts ([D-19](DESIGN-NOTES.md#d-19), [D-23](DESIGN-NOTES.md#d-23),
[D-24](DESIGN-NOTES.md#d-24)) are exactly the kind that are learned by deadlock or by data loss.
This milestone closes that gap with a worked example, not a library -- it demonstrates the pattern
without this crate owning the policy.

The example is a miniature write-ahead log: records appended through the ring, made durable by
group commit, with durability reported by epoch. It is deliberately the shape a real consumer
needs, and it exercises `windows-ioring-sys` and `windows-threadpool-sys` together.

**Depends on M11.1** (`IoRing::completion_event`) and **M12.1** (explicit flush barrier). Do not
start before both have landed; the example cannot be written correctly against the current surface.

- [ ] **M13.1** -- Scaffolding under `examples/epoch_log/`, and the example's **own** durability
  contract written down first, in its own words, per the Design Autonomy rule: *a record is durable
  when the commit of the epoch containing it has completed*. State equally plainly what it does not
  guarantee -- no per-record durability, no ordering between records within an epoch, and no
  atomicity for a record larger than the device's power-fail atomic write unit. The contract is the
  deliverable of this item; the code that follows implements it.

- [ ] **M13.2** -- The append path: a record format with a length, a sequence number, and a checksum
  (so a torn tail is detectable at replay), written into registered buffers and pushed via `Batch`.
  Uses `register_buffers` and `write_registered` deliberately rather than the owned-`Vec` form,
  since an externally-managed buffer arena is what a real consumer has.

- [ ] **M13.3** -- Epoch bookkeeping and group commit: records join the currently open epoch; closing
  epoch *N* pushes a **covering** flush (M12.1) carrying *N* as its identity; observing that flush's
  completion marks every epoch `<= N` durable and releases anything waiting on them. This is the
  construction in "Durability on the ring", and the item is complete when a caller can await
  "epoch *N* is durable" and get a truthful answer.

- [ ] **M13.4** -- The event loop: a caller-owned ring whose `completion_event` (M11.1) is waited on
  by `WaitForMultipleObjects` alongside a shutdown event, draining to empty on **every** pass
  regardless of which handle woke it. This is [D-19](DESIGN-NOTES.md#d-19) in practice, and the
  example must make the drain-to-empty rule visually obvious -- it is the part a reader will
  otherwise get wrong.

- [ ] **M13.5** -- A replay-and-verify pass that reads the log back and checks the contract from
  M13.1 holds: every record reported durable is present and its checksum validates, while records
  after the last committed epoch may be absent or torn and the reader tolerates both. This is what
  turns the example from a demonstration into evidence, and it is the only part of the example that
  can actually catch a durability bug.

## M14 -- Worked example: crossing the ring boundary, and paying for the barrier

Second half of the example. M13 stays inside the ring; this milestone covers the two things that
forced the original consumer conversation -- operations the ring cannot express, and the cost of
[D-24](DESIGN-NOTES.md#d-24)'s full-barrier stall.

**Depends on M13.**

- [ ] **M14.1** -- Order a non-ring operation against ring epochs: an `FSCTL`-class operation issued
  through [`windows-overlapped-io-sys`](../windows-overlapped-io-sys), sequenced at an epoch
  boundary, with its completion waited on in the *same* multiplexed wait as the ring's. This is the
  case `drain_preceding` cannot express at all ([D-24](DESIGN-NOTES.md#d-24) orders SQEs against
  SQEs), and the reason `completion_event` exists. Add the sibling crate as a dev-dependency.

- [ ] **M14.2** -- A control-plane and background path on `windows-threadpool-sys`: checkpointing or
  reclamation driven from the pool while the pinned log thread keeps the data path. Demonstrates the
  hybrid the design notes recommend -- Model B on the hot path, Model A for everything else -- in one
  program, which nothing in the crate currently shows.

- [ ] **M14.3** -- Implement all three epoch-commit strategies from "Durability on the ring" behind
  one interface, selectable at run time: covering flush (ring stalls), host sequencing (a userspace
  round trip per epoch), and alternating rings (neither, at the cost of doubled registration). The
  point is that [D-24](DESIGN-NOTES.md#d-24) makes this a real fork with no free answer, and a
  reader needs to see all three to choose.

- [ ] **M14.4** -- Measure the three strategies on the running machine and print the comparison:
  throughput, commit latency distribution, and ring idle time during the barrier. The example should
  *demonstrate* the trade-off rather than assert it, and the numbers are machine-specific enough that
  quoting ours would be misleading.

- [ ] **M14.5** -- Document the example: a module-level walkthrough, a pointer from `README.md` and
  from the "Durability on the ring" section of [DESIGN-NOTES.md](DESIGN-NOTES.md), and an explicit
  statement that it is a demonstration of a pattern rather than a supported API -- so that nobody
  vendors it and then expects this crate to maintain its policy choices.
