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
  ([D-29](DESIGN-NOTES.md#d-29), fixed in M10.4). Category 2: **every successfully queued SQE produces
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

- [x] **M10.4** -- Gave `FileRef::Registered` safe entry points ([D-29](DESIGN-NOTES.md#d-29)), via a sealed
  `FileTarget` trait with an associated `Guard` type ([D-33](DESIGN-NOTES.md#d-33)) rather than a parallel
  family of `*_registered_file` methods. `read`, `write`, `flush`, `cancel`, `read_registered`, and
  `write_registered` are now generic over it, so a `RegisteredFile` pushes without `unsafe` and the
  file-registered x buffer-registered combination is expressible for the first time. `Guard` carries the
  one real difference between the two targets -- what the `Token` must hold until the completion is
  observed: an `Arc` clone for `SharedFile`, nothing needing to be kept alive for `RegisteredFile` (which
  hands its `Copy` index back for symmetry). **Non-breaking**: `read(&SharedFile, ..)` still resolves to
  `Token<(B, SharedFile)>` and every existing call site compiled untouched; generic params are ordered
  `<B, F>` so an existing `read::<Vec<u8>>` turbofish still resolves. Sealing is load-bearing rather than
  tidiness -- an outside impl could return `FileRef::Raw(arbitrary)` with a guard keeping nothing alive,
  reintroducing what D-16 closed. Three tests: a safe registered-file read, the fully-registered
  composition, and a cross-ring rejection confirming D-17's check still holds on the safe path.

- [x] **M10.5** -- Added named predicates for the ring conditions a consumer must branch on
  ([D-30](DESIGN-NOTES.md#d-30), resolved as [D-34](DESIGN-NOTES.md#d-34)), in three parts. A
  `#[non_exhaustive]` `RingCondition` enum covering **every** `IORING_E_*` this crate names -- not only the
  actionable ones, since narrowing it would be the "narrow the platform to serve the visible goal" failure
  PLATFORM INTEGRITY forbids. Predicates (`is_submission_queue_full`, `is_completion_queue_too_full`,
  `is_submit_in_progress`) for just the runtime-actionable conditions, because a predicate asserts a branch
  exists; the rest stay reachable via `condition()`. And a sealed `IoRingErrorExt` putting those answers on
  `io::Error` itself, which is what actually removes the hand-rolled
  `get_ref().downcast_ref::<IoRingError>()` from call sites -- the crate's own integration tests had two
  such helpers, and deleting them was the new API's first use. `IoRingError::name` is now **derived** from
  `condition()` rather than matching the `HRESULT` a second time (CONTRACT INTEGRITY: prefer a derived fact
  to a restated one) -- previously a new condition could be added to one `match` and silently missed by the
  other. Did **not** map `IORING_E_*` onto `io::ErrorKind`; D-30's refusal stands. Bound to a *real*
  kernel-reported queue-full in the existing backpressure integration test, not only to synthetic errors.

- [x] **M10.6** -- Fixed a live use-after-free in `Batch::register_buffers`
  ([D-32](DESIGN-NOTES.md#d-32)). `BuildIoRingRegisterBuffers` reads its `IORING_BUFFER_INFO` array when
  the registration op *runs*, not when the `Build*` call returns; the crate built that array in a local
  `Vec` and dropped it before `SubmitIoRing`, so the kernel read freed heap and the registration completed
  with `ERROR_NOACCESS`. Since `register_buffers` is a **safe** `pub fn`, safe code could make the kernel
  dereference a dangling pointer -- a soundness hole in shipped 0.1.2, not a test defect. A spike crossed
  array-lifetime against buffer alignment and disproved alignment in both directions; it also established
  that the array may be released once submit returns, and that `BuildIoRingRegisterFileHandles` genuinely
  *does* read synchronously -- the asymmetry the rustdoc had wrongly generalized across, and the reason the
  file-registration tests always passed. The array is now owned by the `IoRing` rather than the `Batch`,
  because a failed submit leaves the SQE queued as ring state ([D-5](DESIGN-NOTES.md#d-5)) and a later
  unrelated submit can be what runs it. Added a regression test that churns the heap between the push and
  the submit, verified by sabotage; corrected both false "read synchronously" claims; moved the entry from
  [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md) to
  [RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md).
