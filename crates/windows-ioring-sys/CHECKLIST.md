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
  **Merge note (2026-08-29):** `main` had meanwhile recorded that the measurement *does* exist -- the
  2026-08-27 session established that a second `BuildIoRingRegisterFileHandles` replaces the whole table and
  re-bases indices at zero, that a table holds at least 65536 handles, and that an index is resolved at
  submission -- and scheduled the follow-up workspace-side as [M19.1](../../CHECKLIST.md), with the
  relaxation of the one-registration-per-ring rule as M19.2. See
  [DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md](../../design-sessions/DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md).
  That does not reopen this item, but it does bound the dissolution: **the dissolution's premise is that a
  second registration is forbidden**, so if M19.2 relaxes that rule, D-14's collision concern becomes live
  again and [D-31](DESIGN-NOTES.md#d-31) must be revisited rather than assumed to still hold. Recorded here
  because nothing else would carry that dependency across the two checklists.

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

## M15 -- Deterministic memory instrumentation (release-gating)

Eight defects came out of the 0.1.x line and the M11-M14 branch:
[#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47),
[#48](https://github.com/MikeGrier/windows-threadpool-sys/issues/48), `get_mut` returning `&mut B`
([D-35](DESIGN-NOTES.md#d-35)), `Appender::claim` leaking an arena slot, the checkpoint path authorising a
reclaim after a failed write, a shared deferred-commit slot in the strategy harness, `get` racing the kernel
([D-36](DESIGN-NOTES.md#d-36)), and a `debug_assert` that could never fire. Sorting them by *what would have
caught them* is what M15-M18 are built from, because they need different tools and one population needs a tool
that does not exist:

- **A -- preconditions never varied.** Every `tests/event_delivery.rs` case handed over a *fresh* ring, so
  "completion queue non-empty at handover" was never a test input. That is
  [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47). M17.
- **B -- failure paths never taken.** No test ever ran `completion.result()` returning `Err`, which is the
  checkpoint defect, and is why [#48](https://github.com/MikeGrier/windows-threadpool-sys/issues/48) surfaced
  as a *lucky* `ERROR_NOACCESS` rather than as corruption. M15 and M16.
- **C -- permissions, not behaviour.** `&mut Vec<u8>` *permits* `reserve`/`resize`/reassign; no execution ever
  performs it. That is [D-35](DESIGN-NOTES.md#d-35) and [D-36](DESIGN-NOTES.md#d-36), the two most severe
  findings, and **no runtime technique reaches this population at all.** M18.

**Deliberately rejected: a mock `IoRing`.** Both shipped defects were the kernel behaving differently from
this crate's assumptions -- the completion event is edge-triggered ([D-19](DESIGN-NOTES.md#d-19)), and
`BuildIoRingRegisterBuffers` reads its array at submit rather than at build ([D-32](DESIGN-NOTES.md#d-32)). A
mock encodes the assumption, so one written before those discoveries would have passed both bugs green -- it
would not merely have failed to find them, it would have manufactured evidence they were absent. A model
belongs here as an **oracle over observed sequences** (M16), never as a substitute for the kernel.

**Deliberately rejected: Application Verifier / PageHeap**, after measuring it rather than assuming either way.
See [D-37](DESIGN-NOTES.md#d-37); the short version is that it works but is keyed by *image file name*, and
cargo rehashes test binaries on every meaningful rebuild.

**M15 and M16 gate the 0.2.0 release; M17 and M18 do not.** 0.2.0 carries two security fixes and should not
wait on the state-space work.

- [x] **M15.1** -- Add a guard-page global allocator for test builds: `VirtualAlloc` each allocation with a
  trailing `PAGE_NOACCESS` guard page, right-aligned so its end abuts the guard, and on free flip the block to
  `PAGE_NOACCESS` and **never reuse the address**. That combination makes both an overrun and a
  use-after-free a deterministic `0xC0000005` instead of a silent stale read.
  Use `MEM_DECOMMIT` rather than plain `VirtualProtect` on free: never reusing an address is the point, but
  never releasing the *commit* would grow a long suite without bound.
  A working probe is at `.scratch/guardalloc/` -- measured clean/UAF/overrun as `0`/`0xC0000005`/`0xC0000005`,
  against a system-allocator baseline that silently read a freed byte and exited `0`.
  **Sabotage-verify against the real defect:** revert [D-32](DESIGN-NOTES.md#d-32)'s fix locally and confirm
  the allocator turns it from a lucky `ERROR_NOACCESS` into a deterministic failure. An instrument never seen
  to fire is indistinguishable from one that cannot.
  **Done:** new `windows-guard-alloc` crate (`publish = false`), installed by `tests/registration.rs`. The
  calibration was run and is the point of the item: reverting [D-32](DESIGN-NOTES.md#d-32) makes
  `a_buffer_registration_survives_heap_churn_between_the_push_and_the_submit` -- the regression test that fix
  added -- die with `STATUS_ACCESS_VIOLATION` instead of passing. Five subprocess tests in `tests/faults.rs`
  prove the allocator fires at all (an access violation cannot be caught in-process), and a
  `the_guard_allocator_is_installed_for_this_test_binary` assertion catches the silent failure mode where the
  `#[global_allocator]` attribute is missing and everything passes uninstrumented.

- [x] **M15.2** -- Fill every allocation and every freed block with a **tracked** poison pattern, derived from
  a per-run seed plus a per-allocation ordinal so the bytes identify *which* allocation they came from rather
  than merely being "not real data". A fixed constant like `0xDD` is worth much less: it collides with real
  payloads, and it cannot answer "where did this come from".
  The tracking is also what keeps this compatible with this component's reproducibility rule: **log the seed at
  test start and accept it from the environment**, so a failure is replayable exactly rather than being a
  one-off. Record that in [DESIGN-NOTES.md](DESIGN-NOTES.md) as the terms under which non-fixed test data is
  permitted here.
  Keep the pattern cheap to write (a repeating 8-byte word carrying the ordinal, not a per-byte hash) since it
  runs on every allocation.
  **Done:** `windows-guard-alloc`'s `poison` module. The pattern is `splitmix64(seed ^ ordinal)` repeated
  across the allocation, and the mixing function is a **bijection with a computable inverse**, so
  `GuardAlloc::poison_check` recovers *which* allocation a region belongs to from its own leading bytes --
  no snapshot needed, which is what M15.3 will build on. Seeding terms recorded as
  [D-39](DESIGN-NOTES.md#d-39) and verified end to end: two unpinned runs differ, two pinned runs reproduce
  byte-for-byte, and both decimal and `0x` hex parse.
  **Two findings while implementing.** (1) *Poisoning freed blocks is dead code here* -- `dealloc` decommits,
  so those bytes are unreadable and poison written there could never be observed by anything. It would cost a
  memset per free for a guarantee strictly weaker than the one already in force, so it is deliberately not
  done, and `dealloc` says why. The item text above is left as originally written rather than quietly edited,
  since the planning error is the more useful record. (2) The first draft **hardcoded two multiplicative
  inverses and both were wrong**; `the_multiplicative_inverses_are_actually_inverses` caught it. They are now
  *derived* by a `const fn` Newton iteration, which makes that class of error unrepresentable rather than
  merely tested for.

- [x] **M15.3** -- Verify the poison at the points where the *kernel* is the suspect, which is the gap guard
  pages structurally cannot cover: a guard page only catches access to memory that should not be touched at
  all, and says nothing about the kernel writing into a live, valid allocation. Both checks below are
  invariants this crate asserts in prose today and verifies nowhere:
  - **After a registered *write* completes** (`KernelAccess::ReadsBuffer`): the slot must be **byte-identical**
    to what was submitted. The kernel is only permitted to read it.
  - **After a registered *read* completes** (`KernelAccess::WritesBuffer`): every byte *outside*
    `[span.offset, span.offset + information)` must still be poison. This binds the kernel to the
    `RegisteredSpan` we declared, and would equally catch our own `checked_span` or offset arithmetic being
    wrong.
  Sabotage-verify each by deliberately submitting a span narrower than the write, and by mutating a slot
  mid-flight.
  **Done:** `tests/kernel_span.rs`, five tests. Both named checks, plus two the item did not anticipate: a
  **short read** must leave the unfilled remainder of its span poison (the span is a permission, not a
  promise, so `information` is the only thing that says where real data stops), and a read into one slot must
  leave its **neighbours** untouched -- which is what per-slot ordinals buy over a single constant, since a
  write landing in the wrong slot is visible as such rather than merely as "something changed".
  All four assertions were sabotage-verified individually and each named the right failure: a byte before the
  span, a byte past its end, a modified write source, and a disturbed neighbour. Every message carries the
  seed, so a failure is replayable per [D-39](DESIGN-NOTES.md#d-39).
  **Note the checks cut both ways**, which is worth more than the kernel-conformance reading: the offset the
  kernel receives is computed by *this* crate in `checked_span`, so an off-by-one there fails these tests
  exactly as a kernel bug would -- and is otherwise as invisible as one.

- [x] **M15.4** -- Verify poison at quiescence too: at teardown every registered buffer's never-written
  regions still hold their expected pattern, and no buffer that was only ever a write source has changed.
  This is the whole-arena form of M15.3 and is what catches a stray write attributed to no particular
  operation.
  **Done:** `windows-guard-alloc`'s `witness` module plus
  `a_mixed_workload_leaves_every_unaccounted_byte_poisoned`. A `Witness` starts owning a region with nothing
  permitted; each completion permits exactly the bytes it was entitled to change -- the **transferred** count,
  never the requested one -- and `verify` walks the *gaps* between merged permissions at teardown. Slot 3 in
  the test is a write source only, so nothing is ever permitted for it and it must be byte-identical.
  Both directions were sabotage-verified: a byte in the gap between slot 0's two disjoint reads, and a byte in
  the write-source slot, each reported with the slot, the offset, expected-versus-found, the seed, and how
  many bytes were legitimately written (`0 byte(s)` for the write source). The 12 `witness` unit tests weight
  the **false-positive** direction deliberately -- abutting ranges, overlapping ranges, out-of-order
  permissions -- because a witness that accuses legitimate writes trains a reader to ignore it, which is the
  same failure as missing one.

## M16 -- Make the contract executable, and take the failure paths (release-gating)

Population B, and the invariants this crate already *states* but nothing checks.

**Depends on M15** only for convenience -- the oracle is independent of the allocator, but the two are most
useful together.

- [x] **M16.1** -- Add `src/contract.rs` with a public `RingContract`: the executable form of
  [the category-2 rule](DESIGN-NOTES.md#one-sqe-one-completion) -- "every SQE that successfully queues
  produces exactly one completion" -- every token claimed-or-deliberately-leaked, and every per-buffer
  outstanding count back to zero at quiescence.
  **Correction (found while implementing):** this item originally cited D-29/D-30 for that rule. Both are
  about something else -- D-29 is `FileRef::Registered`'s safety and D-30 is `io::Error::kind`'s asymmetry --
  and the rule actually lives in the M10.2 audit's category-2 prose, which had no anchor to cite. It has one
  now. The wrong citation had already reached [PLANS.md](PLANS.md) and
  [DESIGN-NOTES.md](DESIGN-NOTES.md) itself; all three are corrected. Fed by the caller (`observe_push` / `observe_completion` / `check_quiescent`) rather than
  wired into `Batch`, so a consumer can validate its own harness against the same definition.
  It lives in **this** crate, not in the test harness: the layer that owns the invariant owns the oracle, and a
  harness-side copy is a second implementation of the rule rather than a check of it. Follow
  [`ContractChecker`](../windows-file-watcher/src/contract.rs)'s shape.
  State plainly what it does **not** check -- it cannot observe device ordering or anything absent from the
  completion stream -- since over-constraining is the same defect as under-specifying.
  **Done:** `src/contract.rs`, 16 unit tests. Five violations: unexpected completion, duplicate completion,
  outstanding at teardown, leaked token, buffer still in use. A completion provisionally marks its operation
  **leaked**, corrected by `observe_claim` -- so forgetting to claim is the default failure rather than the
  silent success, which is the shape of the `Appender::claim` defect. A leak is excused only when *stated*
  via `observe_deliberate_leak`, because the difference between a stated and an unstated leak is the whole
  point. Violations sort before they are reported, since `HashMap` order is unspecified and an oracle whose
  output reorders between runs cannot be diffed. Half the tests are the **does-not-fire** direction, listed
  in the module doc: completion order (the ring promises none between independent operations, and
  [D-24](DESIGN-NOTES.md#d-24)'s barrier constrains execution rather than pop order), operation success,
  anything about the device.

- [x] **M16.2** -- Bind the existing integration tests to `RingContract` and assert quiescence at teardown.
  This is the item that pays for M16.1: the `Appender::claim` slot leak and the strategy harness's shared
  deferred slot were both conservation failures, found by review and by measurement respectively, and both
  would have fallen out of a quiescence assertion automatically.
  Sabotage-verify by reintroducing one of them and confirming the oracle reports it.
  **Done:** bound `tests/submission_lifecycle.rs` (two tests) and `tests/kernel_span.rs`'s mixed workload,
  which also reports per-buffer counts. **The example's `Appender` now owns a `RingContract`**, because the
  claim that the `Appender::claim` leak "would have fallen out automatically" had to be *proved* rather than
  asserted -- and nothing outside the example could prove it. Calibrated against the real defect: taking the
  pre-fix early-return path once makes the run die with `the token for user_data 0x1 was dropped unclaimed,
  leaking whatever it held for the life of the process`.
  **Binding real tests exposed a gap in M16.1**: `flush_raw` and `cancel_raw` return a bare `user_data` with
  no token, so the oracle demanded a claim that cannot be made -- a false violation the caller has no way to
  satisfy, which is the worst kind. Added `observe_tokenless_push` and a `PushedTokenless` state, with four
  more tests. The backpressure test also exercises the rule's other half: the *rejected* push is deliberately
  never observed, since a synchronously-failing `Build*` releases its reservation and produces no completion.
  Noted for M16.3: reverting the fix's *ordering* alone does not reproduce the leak, because the early return
  only triggers on a **failed** write -- which is precisely the population-B path that has no coverage yet.

- [x] **M16.3** -- Add a fault-injection seam at the completion boundary, so a test can make `result()` return
  a chosen error for a chosen operation. `Completion::synthetic` already exists under `#[cfg(test)]` and is
  half of this. Keep it test-gated for the same reason `synthetic` is: `Token::claim_if`'s safety argument
  depends on every `Completion` tracing back to a real popped `IORING_CQE`.
  **Done:** `Completion::with_injected_failure` plus an `InjectedFailure` enum (`Ring` / `Win32` / `Hresult`),
  behind a new default-off `fault-injection` feature.
  **The design turns on a distinction the item's framing missed.** `synthetic` *fabricates* a completion, and
  the reason it must stay `#[cfg(test)] pub(crate)` is not merely convention: a fabricated completion can name
  an operation still **in flight**, so claiming a token against one hands a buffer back while the kernel is
  writing through it -- the exact use-after-free this crate exists to prevent. Building the seam that way to
  test safety would be self-defeating. So this seam **transforms a completion the ring genuinely popped**
  instead: same `UserData`, same ring identity, same finished operation, and only `result()` changes.
  `claim_if`'s argument is therefore untouched, which is what makes the feature safe to expose at all.
  **Found while writing the tests:** `Hresult(0)` would have injected *success*, silently falsifying the
  "failure only, never success" guarantee and letting a test conceal the very defect it was written to find.
  Now enforced by a panic rather than asserted in prose, with a `should_panic` test on it.
  Also deleted a test that could not verify its own name -- `information` is private and unreachable once
  `result()` is `Err`, so "reports no bytes transferred" had no assertion to write. The zeroing stays (it
  would be wrong to model a state the kernel never produces) but the vacuous coverage does not.
  `tests/fault_injection.rs` proves the seam is reachable *and* gated from outside the crate; CI's existing
  `--workspace --all-features` job runs it, verified rather than assumed.

- [x] **M16.4** -- Use M16.3 to cover the failure paths that have no coverage at all: a failed read, write and
  flush completion claimed normally, a failed registration, and `EventDelivery` delivering a failed completion.
  Assert the *documented* degradation each time rather than merely that nothing panics -- the checkpoint defect
  was precisely a failure that was noticed, recorded, and then not acted on.
  **Done:** `tests/failure_paths.rs`, six tests, all five named cases plus the loop-closer M16.2 left open --
  `claiming_before_checking_the_result_is_what_stops_a_failure_from_leaking` runs *both* orderings against
  `RingContract` on the same injected failure, so the `Appender::claim` defect is a reported violation rather
  than an argument. `EventDelivery` uses a **genuine** `ERROR_NOT_FOUND` from cancelling a non-outstanding
  target rather than an injected failure, per M16.3's own guidance.
  Two sabotage-verified: making a failed registration leak instead of drop, and making `EventDelivery` swallow
  failed completions. Both fired with the right message.
  **Corrected M16.3 while doing this.** That item's soundness argument covered `Token::claim_if` only, and
  `PendingBufferRegistration::claim_if` is materially different: it treats a failed completion as proof the
  kernel did *not* retain the addresses, so it **drops the buffers**. Injecting a failure there frees memory
  the kernel genuinely holds registered. Inert only while nothing uses it -- the test takes the precaution
  explicitly and `with_injected_failure` now documents the limitation, which it did not before.
  **Found a race in the test, not the crate:** the `EventDelivery` callback published the error code *after*
  the counter the waiting thread spins on, so the waiter read a stale zero. Fixed by publishing the data
  before the flag -- the same ordering rule the epoch log's reclaim worker already follows.

## M17 -- Feed the detectors M15 and M16 built

Population A. [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47) is one point in a space
nobody was sampling: `{fresh, dirty}` handover state crossed with operation kind, buffer kind, claim/drop and
drain timing. Enumerating that by hand is how it was missed the first time.

**What M15 and M16 changed about this milestone's purpose.** Neither found a single new defect in shipping
code: every finding was either historical ([D-32](DESIGN-NOTES.md#d-32), `Appender::claim`) or a defect in the
new instrumentation itself. That is not evidence the crate is clean. `RingContract`, the guard pages, the
poison, `Witness` and the fault seam are all **passive** -- they fire only on paths the existing hand-written
tests already walk, and #47 lived on a path no test walked. M17 is therefore not a fourth technique standing
beside M15 and M16. It is the **input generator for detectors that are already built and currently under-fed**,
which is also why its cost is lower than when it was first planned: the oracle, the memory checking and the
failure paths all exist now.

**Depends on M16.1** (the oracle is the property these tests assert).

**CONVENTION GATE -- CLEARED.** Randomized property testing is **permitted**, decided by the engineer and
recorded as [D-41](DESIGN-NOTES.md#d-41), so M17.3 is unblocked. Two conditions bind it: seed whatever can be
seeded (so a run replays from one number, and varying that number is how permutations are generated), and
where non-determinism is inherent and outside our control -- multiprocessor scheduling, kernel completion
order -- randomness is fair game. The corollary that keeps the second from becoming a loophole: inherent
non-determinism excuses *irreproducibility*, never *unverifiability*, so a test that cannot be replayed must
instead prove it reached the state it claims to exercise.

- [x] **M17.1** -- Close the two open cells of the handover precondition. A coverage inventory taken after M16
  found this axis is mostly covered already, so this is no longer a matrix to build: *fresh* and
  *queued-non-empty* are covered by [tests/completion_event.rs](tests/completion_event.rs),
  *drained-then-resubmitted* by its `the_edge_re_arms_after_every_drain_to_empty`, and #47's own repro by
  [tests/event_delivery.rs](tests/event_delivery.rs). What remains is (a) attaching while operations are
  **in flight but no completion has landed**, for both `IoRing::completion_event` and `EventDelivery::new`,
  and (b) drain-then-resubmit for `EventDelivery`. Gated on nothing; done first so #47's own axis is banked
  before any convention decision.
  **Done:** [tests/handover.rs](tests/handover.rs), four tests, each bound to `RingContract` so a lost,
  duplicated or unclaimed completion is a contract violation rather than an inferred count.
  **The planned approach for cell (a) was wrong, and its sabotage is what proved it.** The item called for
  racing the attach against in-flight operations. That state does not exist for *buffered* reads: they
  complete **synchronously inside** `submit_and_wait`, measured at 80 of 80 attempts across five shapes up to
  512 reads of 64 KiB, always leaving a full queue at attach time and never a partial split. The first draft
  swept a delay across the supposed window and passed; sabotaging [D-20](DESIGN-NOTES.md#d-20)'s setup signal
  failed it on *attempt 0*, revealing the sweep sat entirely on the already-queued side and the test was a
  slower restatement of `attaching_to_a_ring_whose_queue_is_already_non_empty_still_signals`. Recorded as
  [D-40](DESIGN-NOTES.md#d-40). Cell (a) is covered two ways instead. **Deterministically:** one attach
  serving a backlog queued *before* it **and** a wave submitted *after* it into the still-non-empty queue,
  which is where a seam between the setup signal and the edge would strand work exactly as #47 did.
  **And genuinely in flight:** unbuffered (`FILE_FLAG_NO_BUFFERING`) reads *are* asynchronous, so the state
  is reachable after all -- reusing the aligned-buffer shape [tests/flush_barrier.rs](tests/flush_barrier.rs)
  already had, extended to `IoBufMut`. That test **checks its own precondition**, counting what was already
  queued at the instant of attach and failing if no attempt caught a read in flight, so it cannot decay into
  the already-queued case on faster hardware; dropping only the `NO_BUFFERING` flag makes it fail with
  exactly that message.
  **Calibrated, and the two mechanisms are separable:** suppressing the setup signal fails only the two
  mixed-queue tests; suppressing `EventDelivery`'s `activation.rearm` fails only the re-arm test. Disjoint
  failure sets, so neither test is passing on the other's behalf.
  **Cell (b) was genuinely open, shown by mutation rather than asserted:** with `activation.rearm` removed,
  **all 107 pre-existing tests still pass** -- the entire suite was blind to `EventDelivery` silently
  ceasing delivery after its first drain -- and only the new test catches it. The three targets that did not
  get to run reference no `EventDelivery`, so they could not have caught it either.
  Whether other test files should also bind to `RingContract` is left to **M18.3**'s `cargo-mutants` triage
  to answer with evidence, rather than retrofitted blind now.

- [x] **M17.2** -- Decide and record whether randomized property testing is permitted here, as a
  [DESIGN-NOTES.md](DESIGN-NOTES.md) decision. [D-39](DESIGN-NOTES.md#d-39) already fixed the terms under which
  non-fixed test data is allowed in this component -- **seeded, announced, pinnable** -- and M15.2 proved them
  end to end, so this is now the narrow question "does D-39 extend from poison to `proptest`?" rather than an
  open-ended policy call. Inherit D-39's three terms rather than re-deriving them, and add the two it does not
  cover: a committed regression corpus so any discovered failure becomes a permanent fixed case, and placement
  as **integration** tests rather than unit tests (they cross the OS boundary and will exceed the one-second
  unit budget).
  **Decided: permitted**, recorded as [D-41](DESIGN-NOTES.md#d-41). D-39's three terms carry over to any
  generator, and the two operational terms above are adopted. The engineer added a **second condition D-39 did
  not anticipate**: where non-determinism is inherent and outside our control -- multiprocessor scheduling,
  kernel completion order, thread-pool dispatch -- randomness is fair game, because a test depending on those
  cannot be seeded and pretending otherwise would make it deterministic in its inputs while still
  nondeterministic in what it observes. D-41 records the corollary that stops that becoming a loophole:
  inherent non-determinism excuses *irreproducibility*, never *unverifiability*, so an unreplayable test must
  prove it reached the state it claims to exercise. M17.1 is cited as the worked example of both halves -- its
  racing draft decayed into testing the easier state, and its replacement guards against exactly that.
  No dev-dependency is added here. D-41 permits randomized property testing without mandating a particular
  crate, and M17.3 went on to satisfy its terms with a seeded `SplitMix64` rather than `proptest` -- see that
  item for why.

- [x] **M17.3** -- Model the operation space as data: operation kind, buffer kind (owned / registered), file
  target kind (raw / shared / registered), claim-or-drop, drain-now-or-later, and handover state. A generator
  over *sequences* of these, each sequence checked against `RingContract`. The harness **must run under
  `windows-guard-alloc` with poison enabled**: if it does not, generated sequences are checked for conservation
  only, which discards every M15 detector at exactly the moment there is finally enough input to feed them.
  Address space is not a constraint on doing so -- measured at 8 KiB per allocation, about 8.6e9 allocations
  before 64 TiB, against a generative run needing a few million.
  Bound by [D-41](DESIGN-NOTES.md#d-41): the generator is **seeded, announced and pinnable**, so one number
  replays a whole run and varying it is how permutations are produced. Where a sequence's outcome also turns
  on scheduling or kernel completion order, that residue is legitimately random -- but D-41's corollary still
  applies, so any such sequence must **verify it reached the state it claims**, not merely fail to crash. Note
  the guard allocator's seed and the generator's are two separate knobs and must not be conflated in the
  announcement, or a replay will reproduce one and not the other.
  **Done:** [tests/generated_sequences.rs](tests/generated_sequences.rs). 128 sequences per run, ~650
  operations, all 15 reachable shapes, under the guard allocator and checked against `RingContract`.
  **No `proptest`.** D-41 permits randomized property testing without naming a crate. Its terms are that one
  number replays a whole run and is pinnable from the environment; `proptest`'s model is a persisted
  regression file plus per-case seeds, a different shape, and its headline feature -- shrinking -- earns its
  keep on long sequences. These cap at 10 steps and print every one, so a failure already arrives readable. A
  seeded `SplitMix64` over one `u64` serves the stated terms exactly. If the step cap is ever raised
  substantially, shrinking stops being redundant and the choice should be revisited; that condition is
  recorded in the file's module doc rather than left implicit.
  **Seeding verified end to end:** two runs pinned to the same pair of seeds produced byte-identical coverage;
  a different seed produced different sequences; both decimal and `0x` hex parse. The two seeds are
  independent knobs and both are announced with the replay command.
  **The generator is self-verifying, per D-41's corollary.** A green run proves nothing unless it shows which
  states it reached, so the test asserts every shape appeared, plus at least one deliberate drop, one
  mid-sequence drain, and one attach against a ring with work outstanding (#47's own axis). Sequence count is
  set *by* that assertion rather than by taste: at 48 the rarest shape (flush against a registered file, 3.3%
  of operations) would be missed about 1 run in 4000 and flake CI; 128 puts that at ~3e-10.
  **The first run found a defect in the generated program, not the ring.** It emitted a registered-buffer
  operation whose token was deliberately dropped, and `RegisteredBuffers`'s drop guard fired.
  `read_registered_raw`'s rustdoc already states that token "must be claimed", because claiming is the only
  thing that releases the use -- `Token`'s drop is deliberately empty (D-4), so dropping instead pins that
  buffer index for ever. Claim-or-drop is therefore a free axis only for owned buffers, which the generator
  now encodes alongside the existing rule that two live operations never share one registered buffer index.
  **Calibrated by sabotage, which also exposed a defect in the harness:** removing `RegisteredUse::drop`'s
  `outstanding` decrement -- D-32's own class of bug -- is caught and reported as
  `BufferStillInUse { index: 0, outstanding: 1 }` with the seed and the step that did it. The first attempt
  reported it as a bare `debug_assert` from inside the crate instead, because the registration's drop guard
  fires *while the error is being returned* and masked the diagnostic. The failure path now forfeits the
  registration deliberately -- the same leak-not-free choice the crate makes in release -- so the report
  survives. An instrument whose failure output names neither sequence, step, nor seed is barely an instrument.
  Note this sabotage is a *smoke test* that the harness is not inert, not the calibration M17.4 requires:
  it shows the oracle reports a fault injected into buffer accounting, not that the generator can emit the
  shapes that trigger the defects which actually shipped.

- [x] **M17.4** -- Calibrate the generator before its green result is allowed to count. Show that it
  rediscovers a defect known to be real -- #47's handover shape, or D-32's registration timing -- when the fix
  is reverted. Every M15 and M16 item carried a sabotage step and this milestone had none: a generator that
  cannot emit the shape that triggers #47 reports green for ever, and that green would feed straight into
  release confidence. Until this item passes, M17.3's result is uninformative by construction. Note M18.3's
  `cargo-mutants` run is a general form of the same calibration; this targeted version comes first because it
  is cheaper and aims at two known-real defects rather than synthetic mutants.
  **The calibration failed the generator, which is the entire reason this item exists.** With
  [D-20](DESIGN-NOTES.md#d-20)'s setup signal removed -- #47 exactly as it shipped -- the M17.3 generator
  reported **green**. It attached the completion event but drained with `try_pop`, and unconditional polling
  recovers every completion whether or not the ring ever signalled, so a lost wakeup was invisible to it. A
  generator can sample the right *states* and still be blind to the defect that lives in them.
  **Fix:** once an event is attached, a sequence now **waits for the wakeup it is owed** before draining, and
  a wait that times out with work still outstanding is reported. Waiting precedes draining rather than
  following it, because a backlog queued *before* the attach is reachable only through the setup signal --
  drain-first would consume it by polling and hide the signal's absence. Recorded as
  [D-42](DESIGN-NOTES.md#d-42).
  **Re-calibrated, and the detection is reliable rather than lucky:** with the sabotage restored the generator
  caught it in **10 of 10 runs on fresh seeds**, always within the first six sequences, and the reported trace
  is #47's own shape -- an operation queued, `attach completion event (queue had 1 outstanding)`, then a lost
  wakeup. With the fix in place, 10 of 10 clean runs at ~80 ms.
  **Second defect, D-32:** reintroducing the 0.1.2 use-after-free (build the SQE from an array dropped before
  submit) is caught as a hard `STATUS_ACCESS_VIOLATION`. Worth recording precisely, because it is *not* what
  0.1.2 did: there the same defect surfaced as a survivable `ERROR_NOACCESS` because the freed pages happened
  to still be mapped. The only difference is the guard allocator decommitting them, which is M15.1's stated
  purpose demonstrated against the real historical defect rather than a synthetic one. Note this one is caught
  at the registration *setup* boundary, which every registering test crosses, so it calibrates the allocator
  rather than the generated space -- #47 is the calibration that is specifically about the generator.
  **Process note:** an intermediate "failure" run was traced to a stale test binary left by the previous
  sabotage build, not to flakiness. Timestamps were checked against the rebuild before any conclusion was
  drawn from a direct binary run; a calibration that mistakes a stale artefact for a result is worse than none.

- [x] **M17.5** -- Make discovered failures permanent: every sequence the generator finds becomes a named,
  seed-free regression test committed alongside the corpus. A property test that finds a bug and then forgets
  it has bought a single debugging session rather than a guarantee.
  **The trap in this item is that the generator has found no defect in shipping code**, which makes "nothing
  to make permanent" look like a reason to defer. It is not: the first real failure is exactly the moment
  nobody wants to be inventing a corpus format, so the mechanism is built and proven now.
  **Corpus:** named, seed-free entries replayed verbatim through the generator's own executor, each carrying
  *why* it was recorded. Seeded with `regression_issue_47_backlog_at_handover` -- #47's shape exactly as M17.4
  reported it, kept in the shape it was found rather than tidied into a minimal one, because a corpus that
  rewrites what it was given cannot be trusted to have preserved the failing case. This is a real
  strengthening rather than bookkeeping: the generator reaches that shape within a handful of sequences on
  *most* seeds, and the corpus reaches it on *every* run.
  **Verified load-bearing:** with [D-20](DESIGN-NOTES.md#d-20) reverted the entry fails deterministically,
  reporting "reproduced a defect that was fixed", the recorded reason, and the step trace.
  **The calibration itself is now automated**, which M17.4's could not be. That procedure edits `src/`, so
  nothing in CI would notice if `wait_then_drain`'s wait were later "simplified" back into a poll -- the exact
  change that made the generator blind to #47 ([D-42](DESIGN-NOTES.md#d-42)).
  `the_lost_wakeup_detector_fires_when_no_wakeup_is_owed` constructs the lost-wakeup condition against a
  *correct* ring instead -- attach, consume the setup signal, then wait again without draining, so no
  empty-to-non-empty edge can occur -- and asserts the detector reports it. Sabotaging the wait makes it fail,
  so the guard is real. A short 250 ms budget is used because that wait is *expected* to expire; `WAIT_MS`
  would add five seconds per run to learn the same thing.
  **Found while adding tests:** `temp_file` keyed its fixture on the process id alone, so the second test in
  this binary would have raced the first for one path -- libtest runs them as concurrent threads. Now tagged
  per test.
## M18 -- The population no runtime technique reaches

Population C, and the one that produced the two most severe findings of the M14 review round. `get_mut`
returning `&mut B` ([D-35](DESIGN-NOTES.md#d-35)) and `get` returning an unchecked `&B`
([D-36](DESIGN-NOTES.md#d-36)) are both defects of **what safe code is permitted to do**, not of what any code
path does. No fuzzer, oracle, allocator, poison scheme or chaos harness reaches them, because nothing has to
execute for the hole to exist. Both were found by review, which makes review a *primary* technique here rather
than a backstop -- and that deserves a written procedure instead of depending on who happens to read the diff.

Independent of M17; may run in parallel.

- [x] **M18.1** -- Audit every public type that hands out a borrow or an owned value, against one mechanical
  question: **what can safe code do with this, and does the registration or the kernel still hold anything it
  could invalidate?** `&mut Vec<u8>` permits `reserve`, `resize` and whole-value assignment; `&mut [u8]`
  permits none of them. Record the finding per item even when the answer is "nothing", so the absence of a
  hole is evidence rather than silence.
  **Done:** [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [Borrow-surface audit](DESIGN-NOTES.md#borrow-surface-audit-m181),
  nineteen items, each recorded including the sixteen where the answer is "no hole".
  **One finding, and it is the same shape as the two that prompted the audit.** `EventDelivery::ring` hands
  out `&Mutex<IoRing>`, and *any* `&mut IoRing` permits whole-value assignment, so safe code can replace the
  ring and silently stop delivery. Measured rather than argued: the replacement compiles, and a probe recorded
  one completion delivered before the swap and **none** after, despite four further operations completing on
  the replacement. The pool's wait holds a duplicate of the *original* ring's event, and nothing is ever
  attached to the new one. Recorded as [D-43](DESIGN-NOTES.md#d-43); the fix is **M18.6** below.
  **Worth carrying into M18.2:** the question that finds these is not "is this correct?" but "what else does
  this type allow?" Note also that the two obvious fixes do not work -- a `Deref`/`DerefMut` newtype still
  permits `*guard = ...`, and so does a `with_ring(|ring: &mut IoRing| ...)` closure. Anything that lets a
  `&mut IoRing` escape keeps the hole.
- [x] **M18.2** -- Turn M18.1 into a recurring obligation rather than a one-time pass, as a
  `DESIGN-INSTRUCTIONS.md` rule for this component: any change adding or widening a public borrow-returning
  method answers M18.1's question in the PR. Both C-population defects were introduced by ordinary,
  well-reviewed changes; what was missing was the specific question, not the diligence.
  **Done:** [DESIGN-INSTRUCTIONS.md](DESIGN-INSTRUCTIONS.md) -- the repository's first, so it is kept tightly
  component-scoped. It states the question, what counts as answering it (three points, with the M18.1 audit's
  nineteen rows as worked examples), and that "no hole" is a complete answer that must still be written down.
  **A document alone would not have made this an obligation**, which is the item's own point: the rule it
  replaces was "remember to ask", and that failed three times. So the trigger is mechanical.
  [BORROW-SURFACE.txt](BORROW-SURFACE.txt) is a generated inventory of every public function in `src/` whose
  return type carries a borrow; `tools/check-borrow-surface.ps1` regenerates it and fails on any
  disagreement, wired into CI beside the existing `encoding` and `baseline` sanity jobs -- whose own header
  describes this same restatement-drift problem, so the pattern was already established here rather than
  invented.
  **Scope of the trigger, chosen deliberately:** a reference *or* a lifetime-carrying wrapper. A `&`-only
  scan would have missed `Batch<'_>` and `RingScope<'_>` -- which is precisely where M18.6's fix lives, so the
  narrower scan would have gone blind to the surface it was written for. Seven entries today.
  **Verified load-bearing:** adding `pub fn sabotage_ring(&mut self) -> &mut IoRing` -- D-43's exact shape --
  makes the check fail and print the question. It checks *shape*, not correctness, and the instructions say so
  plainly: it cannot tell a safe accessor from a dangerous one, and tuning it until it stops firing would be
  the failure mode to guard against.
  **Found while writing it:** `.Count` on a `Where-Object` pipeline is a `StrictMode` error when the result is
  empty or a single item -- the check crashed rather than passing on its first clean run. Fixed with `@(...)`.

- [x] **M18.3** -- Run `cargo-mutants` over the crate and triage the surviving mutants. There is a measured
  rate to justify it: this branch produced two vacuously-passing tests (the M11.2 idempotence pair, which
  asserted through a freshly attached event and so could not observe a detached one) and one `debug_assert`
  that could never fire (`Option::take` had already emptied the slot it checked). Both are exactly what a
  surviving mutant looks like.
  **Result:** 306 mutants in 32 minutes -- 182 caught, 48 missed, 7 timeouts, 69 unviable. Counting timeouts
  as detections (a mutant that hangs the suite fails CI as surely as one that trips an assertion), that is
  **189 of 237 viable mutants caught, 79.7%**. Triaged into six categories in
  [MUTATION-SURVIVORS.md](MUTATION-SURVIVORS.md), which is M18.4's input.
  **It found a third vacuous test, which four review rounds had read past.** Both `Arc<[u8]>` tests in
  `src/buf/tests.rs` compare `stable_ptr()` against *another call to the same function*, so returning a **null
  pointer** -- the buffer address this crate hands the kernel -- passes both. They assert self-consistency,
  never that the address is real. The neighbouring `&'static [u8]` test already shows the fix: compare against
  an independently obtained address.
  **And it caught the previous commit's author.** Every read-only accessor M18.6 added to `RingScope` survives,
  because no test calls any of them. The surface was added on the stated principle that a platform layer is
  not narrowed to its current caller, which stands -- but mutation testing showed within one commit that none
  of it is exercised.
  **A first reading of one survivor was wrong, and checking corrected it.** The `contract.rs` mutant deletes a
  match arm in `check_quiescent`, which looked like the oracle's own busy-buffer detection going untested. It
  is the **sort key**, not the detection: the test has one busy buffer, so ordering is unobservable. Recorded
  as equivalent-under-current-tests rather than as a hole.
  **Tooling, measured rather than assumed:** `cargo install cargo-mutants --locked` fails on this host --
  the locked `winapi` does not compile for `aarch64-pc-windows-msvc` (285 errors). Installing unlocked, from
  outside the repo so `rust-toolchain.toml`'s 1.98.0 pin does not apply to the tool, works. `--timeout 180` is
  needed because several mutants hang, and a cap derived from the ~5 s baseline is too tight for a suite whose
  own waits are 5 s. All of this is in the reproduction section of the survivors file.
  Note `cargo-mutants` has no `cargo_*` MCP tool, so it is the rare case where the terminal is the only route.
- [x] **M18.4** -- Resolve the survivors: strengthen the test, or record why the mutant is equivalent and
  harmless. Do not delete an assertion merely because a mutant survives it -- a dead assertion beside a live
  one is worse than none, which is why M14.3's was removed rather than repaired.
  **Result: 48 survivors down to 12, and the score from 79.7% to 94.9%** (219 caught, 12 missed, 6 timeouts,
  69 unviable of 306). 18 new unit tests; no production code changed. Verified by re-running `cargo-mutants`,
  not by assuming the tests would kill what they targeted -- which mattered, because four of them did not.
  **The vacuous `Arc<[u8]>` tests are fixed** by comparing `stable_ptr()` against `Arc::as_ptr`, an
  independently obtained address, exactly as the neighbouring `&'static [u8]` test already did.
  **Four tests failed to kill what they aimed at, and re-running found each one.** Worth recording, because
  every one of them looked correct: (1) the sort-order test used three busy buffers, and with the sort key
  deleted `sort_by_key` is stable, so `HashMap` order came out sorted anyway -- now eight; (2) `checked_index`
  is the *span* bounds check, not `get`'s, so an out-of-range `get` never reached it -- now submits a span;
  (3) `user_data() -> 0` matched because a fresh ring's first operation *is* zero -- now burns ids first, and
  then `-> 1` matched for the same reason, so it burns two; (4) `supports_raw` was asserted only in the
  negative, which a constant `false` satisfies. **A test written against a mutant is not verified until the
  mutant is re-run.**
  **Two survivors are provably unkillable**, argued rather than assumed: `as_hresult`'s `|` versus `^` operate
  on disjoint bit sets and are the same function; `with_injected_failure`'s zeroed `information` is
  unreachable because `result()` returns `Err` first. Recorded with proofs.
  **The rest are recorded with named reasons** in [MUTATION-SURVIVORS.md](MUTATION-SURVIVORS.md), separated
  into unkillable, host-blocked (two `supports -> true` mutants need an `Op` this host does not support, and
  widening a closed enum to satisfy a mutant would be inventing API for a test), queued (capability decoding,
  spawned as **M18.7**), and two `Debug` impls whose exact wording an assertion would only change-detect.
  "Hard to test" is written down as that, never as "equivalent".

- [x] **M18.7** -- Extract the capability flag decoding in `capabilities()` into a pure function over the raw
  `IORING_CAPABILITIES`, so it can be exercised with synthetic flag values. Spawned by M18.4: four mutants on
  `raw.FeatureFlags & FLAG != 0` survive because nothing can vary what `QueryIoRingCapabilities` returns, and
  the decoding of `supports_completion_event` in particular gates every completion-event path in the crate --
  [D-20](DESIGN-NOTES.md#d-20), and #47 lived downstream of it. A pure `fn decode(raw) -> Capabilities` plus a
  table of flag combinations kills all four and makes the one capability that matters testable in both
  directions, which no test can do today.
  **Done:** `decode(&IORING_CAPABILITIES) -> Capabilities` split out of `capabilities()`, which now does the
  syscall and nothing else. Pure refactor -- no behaviour change -- plus five tests, and a round-trip test
  tying `decode` back to the query so the split cannot drift.
  **All four mutants killed**, verified by a scoped re-run: `capability.rs` reports 12 mutants, 9 caught and
  3 unviable, none missed. The killing cases fall straight out of the pure function: an empty mask kills
  `& -> |` (because `flags | FLAG` is non-zero whatever `flags` holds), a mask carrying exactly one named flag
  kills `& -> ^` (which clears the very bit it is testing), and any positive case kills `!= -> ==`. An
  unknown-feature-bit case is included because this crate has already been surprised once by a Windows
  reporting a version it did not name.
  **Crate-wide: 307 mutants, 221 caught, 10 missed, 6 timeouts, 70 unviable -- 95.8%**, up from 79.7% before
  M18.4.
  **Recorded a caveat on the headline number rather than quoting it flat.** Two mutants -- `Batch::require`
  and `IoRing`'s `Debug` -- were reported caught in one run and missed in another *with identical sources*.
  The suite has timing-dependent tests and `cargo-mutants` runs four jobs in parallel, so a mutant can be
  "caught" by a test that merely flaked. Both are now filed as unresolved rather than fixed, and
  [MUTATION-SURVIVORS.md](MUTATION-SURVIVORS.md) says a single run's score is approximate.
  **`Batch::require -> Ok(())` was reclassified on the evidence:** `require` returns `Ok` exactly when
  `supports` is true, so on a host supporting every operation it is equivalent *in practice*. Filed under
  host-blocked rather than provably-unkillable, because the equivalence is a property of the host, not the
  code -- and its single "caught" result is consistent with the flake above.

- [x] **M18.5** -- Document the strategy as a whole in [DESIGN-NOTES.md](DESIGN-NOTES.md): the three defect
  populations, which technique covers which, and -- most importantly -- **what none of them cover.** Two of the
  eight defects came from spikes against the real kernel, and no oracle, generator or allocator would have
  produced either, because both were cases of Windows behaving differently from the assumed contract. Record
  spikes as a budgeted technique for each new Win32 surface rather than something that happens when a test
  mysteriously fails.
  **Done:** [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [Testing strategy](DESIGN-NOTES.md#testing-strategy-m185),
  plus [D-44](DESIGN-NOTES.md#d-44) making the spike rule citable from the decision index rather than reachable
  only through prose.
  **The item's own premise needed a correction, and checking found it.** "No allocator would have produced
  D-32" is not quite true: the guard allocator *does* catch it, as a hard `STATUS_ACCESS_VIOLATION`, measured
  in M17.4's calibration. The accurate statement is sharper and is what got written down -- every technique
  here checks this crate's code against **this crate's stated contract**, and in both spike-found defects the
  stated contract was the thing that was wrong. They detect a *consequence*, on a path some test already
  walks; a spike produces the platform knowledge that decides what the code should be, and is the only
  technique that runs **before there is code to test**.
  **Two things the write-up records that no single milestone did.** First, M15 and M16 finding nothing is not
  reassurance: they are passive, so zero findings meant the detectors had only seen the twenty-odd
  hand-written scenarios that already passed -- which is why M17 exists to feed them. Second, **every one of
  these instruments was wrong the first time and only sabotage found it**: M17.3's generator reported green
  with #47 reintroduced, four of M18.4's mutation-killing tests did not kill their mutant, and M15.2's poison
  inverse was fabricated twice. Budget the calibration, not just the instrument.
  Also records the two rejected techniques -- a mock `IoRing` and PageHeap -- so neither is re-proposed as an
  obvious win, and a per-technique table of what each actually found rather than what it was hoped to find.
- [x] **M18.6** -- Narrow `EventDelivery`'s ring surface so safe code cannot replace the ring out from under
  the pool's wait. Spawned by M18.1, recorded as [D-43](DESIGN-NOTES.md#d-43), and **measured**: the
  replacement compiled and silently stopped delivery -- one completion before the swap, none after.
  **The two obvious fixes do not work.** A `Deref`/`DerefMut` newtype still permits `*guard = ...`, and so
  does handing a `&mut IoRing` to a closure. Closing the hole means never letting a `&mut IoRing` escape at
  all.
  **Done:** `EventDelivery::ring -> &Mutex<IoRing>` is replaced by `EventDelivery::scope -> RingScope`.
  The exposure rule is stated rather than ad hoc: **every read-only part of `IoRing`, plus batch
  construction -- and nothing that can retarget the ring or steal the pool's completions.** So `batch`,
  `outstanding`, `info`, `version`, `supports`, `supports_raw` and the registered counts; deliberately no
  `try_pop` ([D-21](DESIGN-NOTES.md#d-21) makes the pool the single drainer), no `completion_event` (two
  waiters on one ring), no `run_down`, and no `&mut IoRing`.
  **Reviewed for sizing before starting, and one item was right.** Nine call sites across six files, all of
  which stop compiling the moment `ring()` is removed -- so any split would have produced a non-compiling
  intermediate commit, and the add-alongside-then-remove shape would have left the hole open across commits.
  Larger than the item text implied, though: it also forced `handover.rs`'s `submit_wave` to split into a
  `queue_wave` that fills a caller-owned `Batch` (four plain-ring call sites kept their old shape), because
  `Batch::submit_and_wait` consumes the batch and the scope hands out a `Batch` rather than a ring.
  **Proved by construction, not by review**, which is the whole point of M18: a `compile_fail` doctest
  asserts the replacement no longer compiles. That doctest was itself verified -- adding a `DerefMut` impl
  makes it **fail**, so it is passing for the right reason rather than on a typo in its own setup, and that
  same experiment is the empirical proof of the "a `Deref` newtype would not close this" claim above. It is
  paired with a normal doctest sharing the setup, since a `compile_fail` example passes on *any* error.
  **Incidental fix:** call sites were inconsistent about mutex poisoning -- some `expect`, some
  `unwrap_or_else(PoisonError::into_inner)`. `scope()` absorbs poisoning internally, matching what the wait
  callback's own drain already did, so the question no longer reaches callers at all.
  Verified: 148 tests pass, both affected examples (`model_a_delivery`, `epoch_log`) still run to exit 0.