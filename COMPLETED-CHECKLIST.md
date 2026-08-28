# Completed checklist: workspace

Append-only record of completed workspace-level checklist groups.

## Moved 2026-08-16 — Workspace and release (M1)

- [x] Specialize the crate name, metadata, documentation, and release config.

- [x] Split the workspace into `windows-overlapped-io-sys` and `windows-threadpool-sys` with independent,
	component-tagged publishing.

- [x] Reserve the `windows-overlapped-io-sys` name on crates.io — published `windows-overlapped-io-sys` and
	`windows-threadpool-sys` v0.1.0 to reserve both names.

- [x] Confirm CI and crates.io publishing secrets are configured for both crates.

## Moved 2026-08-16 — Shared invariants (M2)

- [x] Select the initial `windows-sys` feature set and document the FFI boundary.

- [x] Choose the minimum supported Windows version for the pair (Windows Server 2025 / Windows 11, per CI).

- [x] Specify the ownership, cancellation, and callback lifetime invariants shared by both crates — see the
	"Shared invariants (both crates)" section in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## Moved 2026-08-17 — M3 operation identity must not alias a recycled operation

An `OperationId` was only the address of an operation's storage. Reclaiming an operation returns that address to
the allocator, so a later operation could be handed it, and an identity retained from the earlier operation then
named the later one. Because `AssociatedEndpoint::cancel` and `ThreadpoolIo::cancel` are **safe** functions
acting purely on that address, a stale identity could silently cancel an unrelated live operation. Reproduced
directly: cancelling and draining an operation, then submitting a fresh one, recycled the identity within 64
cycles.

- [x] **AB-1** — Give every submitted operation a process-wide monotonic generation, carry it in `OperationId`,
	and have both backends keep a live-identity registry that cancellation validates against.
	*(completed 2026-08-17 16:26:00 -04:00)*

	`windows-overlapped-io-sys` gained `identity.rs` (the global generation counter, the widened `OperationId`,
	and the shared `OperationRegistry`). Both backends replaced their outstanding counters with the registry, so
	the count and the liveness set cannot disagree. A stale or unknown identity is rejected with
	`ErrorKind::NotFound` **without** calling `CancelIoEx`. Deliberately reversed the IOCP backend's documented
	lock-free-submission property, and made `OperationId` `Send + Sync` — without which cancelling from another
	thread, the whole point of holding an identity, was impossible.

- [x] **AB-2** — Test in `windows-overlapped-io-sys` that a retained identity cannot cancel a recycled
	operation on the IOCP backend, including a direct reproduction of address recycling.
	*(completed 2026-08-17 16:30:00 -04:00)*

	8 integration tests. They assert the registry rejected the identity *before* any native call (a registry
	rejection carries no OS error code, a `CancelIoEx` one always does), which is what proves a recycled address
	was never handed to the kernel.

- [x] **AB-3** — Test in `windows-threadpool-sys` that a retained identity cannot cancel a recycled operation
	on the `TP_IO` backend, and that live identities still cancel normally.
	*(completed 2026-08-17 16:38:00 -04:00)*

	9 integration tests covering stale rejection, cross-object rejection, double cancellation, cross-thread
	cancellation, and identity/completion matching at scale.

	The registry's duplicate-address assertion caught a **pre-existing race in the M3-2 `TP_IO` implementation**:
	it deregistered an operation after running the callback, but `IoCompletion::claim` frees the storage inside
	that callback, so a concurrent submission could be handed the address while the completed operation was still
	registered. Fixed by deregistering on callback entry; `run_down` then also waits for callbacks so it keeps
	its "my callbacks have run" contract. Verified the regression test fails 10/10 with the fix reverted.

## Moved 2026-08-17 — PR #3 review findings

Six review threads, all verified against the code before being accepted. Four were undefined-behaviour paths
reachable from safe code, and two of those undermined guarantees this same branch introduced -- the identity
work fixed one instance of an aliasing hazard while leaving other routes to it open.

- [x] **PR-1** — Make cancellation validate and act under one lock, and route every backend through it.
	*(completed 2026-08-17 18:00:00 -04:00)*

	`cancel` checked liveness with `is_live` and then called `CancelIoEx` after the mutex was released; a
	completion could reclaim the operation and a concurrent submission reuse its address in that window.
	`OperationRegistry::cancel_if_live` now holds the guard across both steps. `AssociatedSocket::cancel` never
	consulted the registry at all, so socket identities bypassed the guarantee entirely.

- [x] **PR-2** — Compare full operation identities in the typed claim tokens.
	*(completed 2026-08-17 18:03:00 -04:00)*

	`FileIo`, `ScatterGatherIo`, `SocketIo`, and `DeviceIoControlIo` matched a completion by address only, so a
	token outliving an unclaimed completion could match a later completion that reused the address and claim it
	with the wrong payload type -- type confusion reachable without `unsafe` on the caller's side. The `SAFETY`
	comments asserted the address match proved the payload type, which it did not.

- [x] **PR-3** — Make `CallbackEnviron` actually retain the pool borrow it appears to take.
	*(completed 2026-08-17 18:06:00 -04:00)*

	`set_pool` took `&ThreadpoolPool` but stored only the raw value, with no lifetime on the environment, so
	safe code could drop the pool and then create an object from the still-live environment. The environment now
	carries the pool's lifetime, pinned by a `compile_fail` doc test.

- [x] **PR-4** — Contain panics in the work trampoline.
	*(completed 2026-08-17 18:06:00 -04:00)*

	The `TP_WORK` trampoline invoked the callback without `catch_unwind`, unlike every other trampoline, so a
	panicking work callback aborted the process instead of being contained as documented.

- [x] **PR-5** — Defer token-requested timer re-arming until the callback has returned.
	*(completed 2026-08-17 18:10:00 -04:00)*

	`TimerFiring::rearm_after` armed immediately, so a callback that re-armed early and then ran longer than its
	delay could be entered again concurrently. The request is now applied after the callback returns. Arming
	from outside during a callback can still overlap, so the type documents the guarantee it actually provides.

## Moved 2026-08-17 — M5, second review round on PR #3

Four review findings, reducing to two work items: each was reported twice, once against the individually-owned
object and once against the second path reaching the same hazard. Both are cases where a precondition was
written down instead of enforced. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for both decisions.

### <a id="rv-1"></a>RV-1 — Require a wait handle whose provenance is established safely. *(completed 2026-08-17 19:07:57 -04:00)*

`ThreadpoolWait::new` and `CleanupGroup::create_wait` were safe functions taking any `OwnedHandle`, while the
documentation admitted that passing a mutex handle is unsupported by the thread pool and therefore undefined. A
safe function cannot delegate a precondition its caller can trivially violate.

Both constructors now take a `WaitableHandle`, mirroring the shape `UnassociatedEndpoint` uses in the sibling
crate: safe constructors for handle kinds this crate creates itself, plus one narrow `unsafe assume_waitable`
seam for handles obtained elsewhere. Taking it in both constructors is what stops the cleanup-group path from
remaining unsound. The wait doctests no longer need `unsafe` to build their events.

### <a id="rv-2"></a>RV-2 — Gate re-arming against teardown in both the timer and the wait. *(completed 2026-08-17 19:07:57 -04:00)*

A callback could arm either object *after* `Drop` had disarmed it -- directly in `WaitActivation::rearm`, and
via the deferred `PendingRearm` the timer's trampoline applies once the callback returns. The drain could then
complete with a due time installed, and the object be closed and its context freed with a fresh callback queued
against it. The timer half was a window the previous round's fix introduced.

Both contexts now carry a `shutting_down` flag; arming takes it and no-ops when set, and `Drop` sets it and
disarms under one acquisition. The lock is never held across the callback drain, which would deadlock a
callback blocked on it.

Suppression is not observable from outside, so each object exposes the outcome to its own tests
(`rearm_reporting`; a test-only observer on the timer). Both regression tests were confirmed to fail with the
gating removed -- an earlier version that only asserted teardown terminated passed either way.

## Moved 2026-08-17 — M6, third review round on PR #3

Five findings, raised as suppressed comments and all verified against the code before being fixed. Two were
functional defects in safe APIs, three were documents that no longer described the code beside them.

Both defects had the same shape, recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md): each guard tested the condition
that had been *written down* rather than the one that actually mattered.

### <a id="rw-1"></a>RW-1 — Reject periods below one millisecond in `ThreadpoolPeriodicTimer`. *(completed 2026-08-17 20:52:23 -04:00)*

`new` rejected only a zero period, but every arming converts with `Duration::as_millis()`, flooring anything
under 1ms to zero -- which `SetThreadpoolTimer` reads as "do not repeat". Measured: 999us fired **once** in
300ms, 1000us fired 31 times, with no error. Rejection now sits at a public `MIN_PERIOD` constant, mirrored on
`CleanupGroup::create_periodic_timer`. The regression test asserts that the shortest accepted period actually
repeats, so lowering the constant makes it fail by timing out.

### <a id="rw-2"></a>RW-2 — Release cleanup-group members created after a previous release. *(completed 2026-08-17 20:52:23 -04:00)*

`release_members` latched a `released` flag. Since `create_*` take `&self`, members could arrive after a release
returned and were then skipped by both a later `close_members` and by `Drop`: measured `owned_resources` still 1
after a second close, leaking the context and closing the group with a live member. The flag was removed rather
than reset, since the native release is idempotent -- which fixes the leak and makes the group reusable instead
of merely rejecting reuse.

### <a id="rw-3"></a>RW-3 — Correct the wait-handle provenance claim in the crate README. *(completed 2026-08-17 20:52:23 -04:00)*

The safety summary still said `ThreadpoolWait` takes an `OwnedHandle`; it takes a `WaitableHandle`, which is
what keeps unsupported handles out of the safe API. The periodic period floor was added to the same list.

### <a id="rw-4"></a>RW-4 — Remove the stale `SESSION-CONTEXT.md` snapshot. *(completed 2026-08-17 20:52:23 -04:00)*

A design-phase snapshot added on this branch, stating that no safe API had been implemented and that the safety
boundary was unresolved -- both false by the end of the PR. Every contract it recorded was verified present in
the two DESIGN-NOTES files before removal.

### <a id="rw-5"></a>RW-5 — Update the operation-identity seam decision to the API that exists. *(completed 2026-08-17 20:52:23 -04:00)*

The decision still justified `OperationId::from_ptr`, removed by the generation-stamped redesign. It now
describes `mint` and `from_parts`, why an identity carries a generation, and the aliasing failure that motivated
it.

## Moved 2026-08-17 — M7, fourth review round on PR #3

Ten findings, verified against the code before being planned. Nine were correct as stated; one named the wrong
method, and the measurement is recorded below rather than glossed over.

Several were one recurring defect: a value accepted by a safe API and then not honoured -- a period that rounds
away, a buffer length that truncates, a counter that wraps. Each is fixed the same way, by rejecting what cannot
be honoured, so the API never returns an object that does something other than what it was asked for. That theme
is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md).

### <a id="tr-1"></a>TR-1 — Give the timer and the wait a real stop-and-drain. *(completed 2026-08-17 21:45:42 -04:00)*

The review claimed `cancel_pending` cannot quiesce a self-re-arming object. Measured, it can: `disarm()` +
`cancel_pending()` is quiescent, as is `cancel_pending()` alone. The method that fails is `wait()` -- after
`disarm(); wait();` a self-re-arming timer was still set and fired four more times -- while its documentation
promised exactly that quiescence.

`cancel_pending`'s success also depends on the pool dropping a callback armed by the trampoline during an
in-flight cancel, which no SDK contract promises. Both types now have `stop_and_drain`, suppressing re-arming
under the same lock `Drop` uses and lifting it before returning, matching the `ThreadpoolPeriodicTimer` method
that already existed. The suppression became a depth count so concurrent callers and a later `Drop` compose.

### <a id="tr-2"></a>TR-2 — Reject periods a periodic timer cannot honour exactly. *(completed 2026-08-17 21:45:42 -04:00)*

M6 added a lower bound; `as_millis()` still truncated a fractional period (1.5ms scheduled at 1ms while
`period()` reported 1.5ms) and capped anything beyond `u32::MAX` ms. Both now rejected, with a `MAX_PERIOD`
constant.

### <a id="tr-3"></a>TR-3 — Fail generation minting at exhaustion instead of wrapping. *(completed 2026-08-17 21:45:42 -04:00)*

`fetch_add` wrapped at `u64::MAX` and reissued generations from zero, against a type documenting uniqueness for
the life of the process. Minting now panics, stickily -- the counter is pinned, since `fetch_add` has already
wrapped it to zero by the time exhaustion is detectable. The counter is a parameter so the boundary is testable.

### <a id="tr-4"></a>TR-4 — Reject ioctl buffers too large for the Win32 length field. *(completed 2026-08-17 21:45:42 -04:00)*

`clamp_u32` capped at `u32::MAX`, submitting a prefix of the caller's input and reporting success. Both entry
points validate before allocating; the submitting path measures lengths up front because its closure runs at the
FFI boundary and cannot report errors.

### <a id="tr-5"></a>TR-5 — Gate `windows-threadpool-sys` behind `cfg(windows)`. *(completed 2026-08-17 21:45:42 -04:00)*

Measured rather than inferred: `cargo check --target x86_64-unknown-linux-gnu` gave 5 errors for the threadpool
crate and 0 for the sibling. After gating, the whole workspace including test targets gives 0, and the root
README's claim is true.

### <a id="tr-6"></a>TR-6 — Correct two API documents. *(completed 2026-08-17 21:45:42 -04:00)*

`ThreadpoolIo::new` offered a cleanup group the design deliberately excludes; `CallbackEnviron::clear_pool`
claimed to drop a borrowed pool.

### <a id="tr-7"></a>TR-7 — Archive the completed plans per the repository's own convention. *(completed 2026-08-17 21:45:42 -04:00)*

Both crates' checklists sat in the active `PLANS.md` tables marked completed, with no `COMPLETED-PLANS.md`
anywhere. Created at both crate level and the workspace root; the root's own row is marked in progress, which it
actually was.

### Also in this milestone

A stress scenario from M5, `stress_one_shot_arm_and_await_fire`, was found failing during the milestone gate. A
worktree at the previous commit failed the same way, so it was pre-existing rather than caused by TR-1: it
recorded its firing count while still holding the overlap guard, releasing the driving thread mid-callback so it
armed into an overlap that is permitted for external arming. The test, not the timer, was wrong.

## Moved 2026-08-17 — M8, fifth review round on PR #3

Two review findings, both verified, and both in code this branch introduced -- one of them in the previous
round's fix. A third item was added mid-milestone after the second finding turned out to have an unreported
twin. Decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md).

### <a id="fr-1"></a>FR-1 — Close the wrap window in the generation sequence. *(completed 2026-08-17 22:22:19 -04:00)*

[`TR-3`](#tr-3) replaced a wrapping `fetch_add` with `fetch_add` plus a `store` to pin the counter. Those are
two operations and the counter is already wrapped to zero between them, so a thread arriving in that window
takes 0, then 1, 2, ... and mints successfully -- the exact aliasing the guard existed to prevent. A single
saturating `fetch_update` replaces both, so the counter never transiently holds a wrapped value.

The first regression test tried to catch a thread minting a recycled generation and passed against the broken
implementation, the window being a few instructions wide. It now watches the counter itself, and fails with
`the counter held a wrapped value (0)`.

### <a id="fr-2"></a>FR-2 — Require exclusive access for the safe blocking adapters. *(completed 2026-08-17 22:22:19 -04:00)*

`BlockingEndpoint` is automatically `Send + Sync`, and its five safe adapters took `&self` while calling an
`unsafe` `run` whose contract forbids a second outstanding operation. Two threads could therefore each have an
`OVERLAPPED` in flight; `run` waits on the handle, which either completion signals, so a call could return the
other's result and free buffers the kernel was still using. Safe code could reach it.

The adapters now take `&mut self`, making it a borrow-check error. Pinned by a `compile_fail` doctest paired
with a positive control differing only in single ownership versus an `Arc`, so the rejection is demonstrably the
borrow requirement rather than any compile error.

### <a id="fr-3"></a>FR-3 — Reject file and scatter/gather lengths too large for the Win32 field. *(completed 2026-08-17 22:22:19 -04:00)*

Not from the review. Found while working FR-2: `fs.rs` carried its own copy of the capping helper that
[`TR-4`](#tr-4) removed from `device.rs`, across eight call sites. The scatter/gather adapters reach the limit
through a page count, and now check before allocating, which also converts `PageBuffers::new`'s overflow panic
into an ordinary error.

## Moved 2026-08-17 — M9, sixth review round on PR #3

Five findings, all verified and all correct. Three were in work this branch introduced in the two preceding
rounds, including one that is a direct failure to act on a lesson written into the previous round's own design
note. Decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md).

### <a id="fz-1"></a>FZ-1 — Document that re-arming a still-signalled wait overlaps its own callback. *(completed 2026-08-17 23:00:36 -04:00)*

`WaitActivation::rearm` said nothing about concurrency. On a manual-reset event the handle stays signalled, so
re-arming from inside the callback queues the next activation before the current one returns. Measured:
re-arming at the top of a 20ms callback entered it **7529 times in 400ms, 5110 of those overlapping**, against 1
entry and no overlap for an auto-reset event. Documented on both the method and the type, contrasted with
`TimerFiring::rearm_after`, and pinned by three tests.

Writing the mitigation exposed that it was unreachable -- the advice is to reset the event before re-arming, but
nothing exposed the handle to the callback -- so `WaitActivation::handle` was added.

### <a id="fz-2"></a>FZ-2 — Validate file read lengths before allocating. *(completed 2026-08-17 23:00:36 -04:00)*

[`FR-3`](#fr-3) put the check after `vec![0_u8; len]` in the blocking read, so an oversized request tried to
allocate over 4GiB before failing and its own regression test could abort instead of exercising the error path.
The IOCP read was already correct.

### <a id="fz-3"></a>FZ-3 — Reject oversized socket lengths. *(completed 2026-08-17 23:00:36 -04:00)*

`socket.rs` held a **third** copy of the capping helper, four call sites across both backends, allocating before
capping. Same defect as `device` and `fs`. No capping helper now remains in either crate; the one surviving
saturation, a coalescing window, is deliberate and documented as such.

Also checked `BlockingSocket` for the exclusivity hole fixed in [`FR-2`](#fr-2): it does not have it, because
its `run` creates a fresh event per call rather than waiting on the shared socket.

### <a id="fz-4"></a>FZ-4 — Stop the identity tests mutating global state, and remove their false-pass mode. *(completed 2026-08-17 23:00:36 -04:00)*

Two defects in one test. Four worker threads each swapped the *process-global* panic hook, which can leave the
no-op hook installed and strip diagnostics from every other test in the binary; and the observers could be
scheduled after the minters finished, sampling nothing and passing against the broken implementation.

Fixed by a non-panicking `try_next_generation` seam -- so the concurrent test raises no panics and touches no
hook -- and a barrier so observers are provably running before the boundary is crossed. Verified to still fail
four times out of four against the broken implementation, and pass four out of four with the fix.

## Moved 2026-08-17 — M10, seventh review round on PR #3

Six findings, four of which were one claim -- and that claim is the first in this PR to be **rejected on
evidence** rather than fixed. Decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md).

### <a id="ga-1"></a>GA-1 — Record that the scatter/gather 64 MiB limit does not exist. *(completed 2026-08-17 23:23:05 -04:00)*

The review asserted, in four separate comments, that `ReadFileScatter` and `WriteFileGather` have a documented
per-call ceiling of 2^26 bytes and that all four scatter/gather paths should reject anything larger.

Checked twice, negative both times. Both Microsoft Learn pages were read in full and neither states any per-call
byte ceiling. Measured directly on this machine: scatter reads of 16383, 16384, 16385 and **32768** pages all
succeeded, the last returning 134,217,728 bytes -- 128 MiB, twice the claimed limit.

No length change was made. Implementing the suggestion would have rejected requests the platform accepts,
introducing a defect while appearing to remove one. The investigation and its evidence are recorded so the claim
is not re-raised and nobody later "fixes" its absence.

### <a id="ga-2"></a>GA-2 — Name the `LongFunction` flag instead of writing its bit inline. *(completed 2026-08-17 23:23:05 -04:00)*

`CallbackEnviron::set_runs_long` ORed a bare `1` into the environment's flags word -- the manifest identity of an
ABI bit, which this repository's conventions forbid inline. Now an `environ_flags::LONG_FUNCTION` constant,
declared once with a note that changing it is a breaking change. The behavioural tests deliberately keep their
literal `1`, since asserting against the constant would pass even if the constant were wrong; a separate test
pins the constant itself.

### <a id="ga-3"></a>GA-3 — Bring the pull request's breaking-changes list up to date. *(completed 2026-08-17 23:23:05 -04:00)*

The list still described only the original PR after six rounds of hardening. Rebuilt from the commit history
rather than memory, split into signature changes, inputs now rejected rather than silently altered, and additive
items worth knowing. The validation section was refreshed too (285 tests to 363, plus the non-Windows build and
the opt-in stress suite).

## Moved 2026-08-18 — M11, eighth review round on PR #3

Three review findings plus one found while validating them. The first was resolved by **correcting the claim
rather than the code** -- an owner decision, recorded as such rather than presented as a fix. Decisions in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

### <a id="ha-1"></a>HA-1 — Stop `stop_and_drain` promising quiescence it cannot enforce. *(completed 2026-08-18 00:03:36 -04:00)*

The four `stop_and_drain` methods suppress a *callback's* re-arm under a lock, but the external arming methods
take `&self` on `Sync` types and bypass it, so a concurrent arm inside the stop window is not excluded by
anything in this crate -- while the documentation stated flatly that the object was idle on return.

No observable failure could be produced, and the reason matters: `WaitForThreadpoolTimerCallbacks` with
cancellation was measured clearing a due time even with no callback queued (`is_set` true then false), where
`wait()` leaves it set. The drain cancels a racing arm incidentally -- the same undocumented behaviour this crate
had already declared it would not depend on.

Owner decision: add neither a lifecycle gate nor forced exclusive access, and correct the documentation instead.
Each method now separates what it enforces from what it assumes, the assumption is stated with the way to
satisfy it, and the measurement is recorded so the incidental cancellation is not later mistaken for a contract.
All four methods, the type docs and the design note were corrected, not just the one line the review quoted.

### <a id="ha-2"></a>HA-2 — Detect page-count overflow instead of saturating it. *(completed 2026-08-18 00:03:36 -04:00)*

`pages.saturating_mul(PAGE_SIZE)` defeats its own validation on 32-bit Windows, where `usize::MAX` *is*
`u32::MAX`: an overflowing count saturates into a value the length check accepts, and `PageBuffers::new` then
panics instead of the adapter returning `InvalidInput`. Both scatter-read paths now share a checked
`scatter_gather_len`.

### <a id="ha-3"></a>HA-3 — Prove the identity observers sampled before the boundary is crossed. *(completed 2026-08-18 00:03:36 -04:00)*

The single barrier proved only that each observer had *reached* it; the scheduler could still run every minter
and the stop store before an observer looped once, which both removed all detection power and tripped the
sampled-at-least-once assertion, making the test fail at random. A second handshake, passed only after each
observer has sampled, makes the precondition hold. Still detects the broken implementation 5 times out of 5, and
passes 10 out of 10 with the fix.

### <a id="ha-4"></a>HA-4 — Fix a wait test that races the overlap it now documents. *(completed 2026-08-18 00:03:36 -04:00)*

Not from the review. Found by re-running the suite while validating HA-1: `rearming_outside_teardown_is_honoured`
failed once in twenty runs. It took its "first activation only" branch on a non-atomic `count() == 0` while
watching a manual-reset event -- which stays signalled, so the re-arm queues the next activation immediately and
two callbacks could both observe zero. That is exactly the overlap documented in [`FZ-1`](#fz-1), latent in this
test since the round that introduced it. The activation is now selected atomically: 25 runs clean, and 12 full
workspace runs clean afterwards.

## Moved 2026-08-18 — M12, ninth review round on PR #3

Seven findings, all verified and all correct. Two were overclaims of exactly the kind the previous round was
spent removing, and two were violations of this repository's own documented conventions. Decisions in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

### <a id="ib-1"></a>IB-1 — Reject a zero thread maximum, and correct what the maximum actually does. *(completed 2026-08-18 00:25:15 -04:00)*

Both parts measured, since the SDK page states neither. `set_max_threads(0)` leaves a pool that runs nothing --
a submitted work item did not execute in three seconds, and `SetThreadpoolThreadMaximum` returns void so nothing
could report it. And the documented claim that "the pool clamps the value to at least the current minimum" is
false: a minimum of 4 followed by a maximum of 2 peaked at **2** concurrent callbacks, so the maximum wins.

Owner decision: reject zero, returning `io::Result<()>` as `set_min_threads` already does. The clamping sentence
is replaced by the measured behaviour, which is now pinned by a test -- as is the rejection. An existing unit
test carried the false claim in its *name* (`max_below_min_is_clamped_not_rejected`) and was renamed to describe
what it actually checks.

### <a id="ib-2"></a>IB-2 — Scope the "never overlaps" claim to callback-driven re-arming. *(completed 2026-08-18 00:25:15 -04:00)*

The crate overview and README both stated flatly that a `ThreadpoolTimer` never overlaps, while the type's own
documentation has a *When firings can overlap* section saying the opposite for external arming. A reader
choosing between the timer types from the overview was being given the wrong basis for the choice. Both now
scope the guarantee to re-arming through `TimerFiring` and name the exception.

### <a id="ib-3"></a>IB-3 — Correct the `set_pool` migration note in the pull request description. *(completed 2026-08-18 00:25:15 -04:00)*

The breaking-changes list said `set_pool` "takes an owned `ThreadpoolPool`". It takes a *borrow*, deliberately
retained as `CallbackEnviron<'pool>` -- which is the entire point of the change. As written it told a consumer
to hand over ownership they must in fact keep. Corrected, and the new `set_max_threads` break added.

### <a id="ib-4"></a>IB-4 — Make the planning documents obey the repository's own rules. *(completed 2026-08-18 00:25:15 -04:00)*

[COMPLETED-PLANS.md](COMPLETED-PLANS.md) referred to `COMPLETED-CHECKLIST.md` as inline code rather than a
clickable relative link; it now links all three archives, each verified to exist. Both crates' CHECKLIST files
carried a paragraph summarising everything the crate covers -- historical prose duplicating the archive, in
files this repository defines as action-only. Removed.

## Moved 2026-08-18 — M13, tenth review round on PR #3

Two findings, both correct. Decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) and
[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md).

### <a id="jc-1"></a>JC-1 — Make an operation identity unforgeable by safe code. *(completed 2026-08-18 00:54:28 -04:00)*

`OperationId::from_parts` was safe and took any generation, so safe code holding `(p, g)` could construct
`(p, g + 1)` and, if the next submission reusing `p` were stamped with that generation, cancel an operation it
never submitted. The method's own documentation claimed the opposite. An isolation break rather than undefined
behaviour: cancelling a live operation is well-defined and tokens are not forgeable.

Fixed by removing the pairing step from the normal path rather than guarding it. Every caller was reassembling
what the registry had just returned, so `OperationRegistry::remove` and `identify` (formerly `generation_of`)
now return a whole `OperationId`, and both backends store the identity rather than a bare generation. Safe code
has no way to pair an address with a chosen generation at all.

`unsafe fn forge` remains for the tests that prove a stale or ahead identity is *rejected* -- coverage that has
to be reachable from the sibling crate, where a `pub(crate)` seam would not be. A `compile_fail` doctest proves
safe code cannot forge, paired with a positive control differing only in the `unsafe` block so the rejection is
demonstrably the missing obligation.

### <a id="jc-2"></a>JC-2 — Correct the pool-lifetime claim in the pull request description. *(completed 2026-08-18 00:54:28 -04:00)*

The description claimed `CallbackEnviron<'pool>` "makes the compiler enforce that the pool outlives the objects
created from it". It enforces that against the *environment*; an environment's contents are copied into each
object at creation, so no reference survives for the compiler to follow. `ThreadpoolPool` documents this
accurately under *Ordering requirement* -- only the description was wrong, and it was written in the previous
round while correcting a different description error.

That makes four consecutive rounds in which summary prose overclaimed something the reference documentation
states correctly, so the pattern itself is now recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) with a rule for
writing such prose, rather than being fixed one sentence at a time.

## Moved 2026-08-18 — M14, eleventh review round on PR #3

Two findings, both correct, and both introduced by the previous round. Decisions in
[DESIGN-NOTES.md](DESIGN-NOTES.md) and
[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md).

### <a id="kd-1"></a>KD-1 — Remove the form-feed characters, and make the encoding check able to see them. *(completed 2026-08-18 01:19:11 -04:00)*

A PowerShell replacement in the previous round contained `` `forge` `` in a double-quoted string, and PowerShell
read the backtick-`f` as its form-feed escape, committing `<FF>orge` into a source comment.

Wider than reported: the review named the thread-pool test, but the same replacement ran over the overlapped
crate's test, which had identical damage. A repository-wide byte scan found these two and no others.

The guard missed it. [tools/check-encoding.ps1](tools/check-encoding.ps1) tested only for invalid UTF-8 and
mojibake digraphs, and a form feed is neither, so CI passed both damaged files. It now rejects any C0 control or
DEL other than tab, line feed and carriage return, reporting byte value and line -- verified against a planted
form feed rather than only against the repaired files.

### <a id="kd-2"></a>KD-2 — Make the operation-identity decision describe the API that exists. *(completed 2026-08-18 01:19:11 -04:00)*

The seam paragraph still introduced `OperationId::from_parts` as the escape hatch and said both constructors were
safe, while a subsection immediately below explained that safe assembly is forbidden -- so the canonical decision
contradicted both the API and itself.

This was the second time that paragraph went stale the same way: [`RW-5`](#rw-5) corrected it when `from_ptr`
was replaced, and the two accounts drifted again. Rather than patch it a third time, the subsection was folded
into the paragraph so there is one account to keep current. Applying the check-for-siblings rule also found the
same stale claim in the workspace [DESIGN-NOTES.md](DESIGN-NOTES.md), which the review had not flagged;
`COMPLETED-CHECKLIST` mentions were left alone, being append-only history that was accurate when written.

## Moved 2026-08-18 — M15, twelfth review round on PR #3

### <a id="le-1"></a>LE-1 — Restore the doc-comment separator glued to a code line, and make CI reject the pattern. *(completed 2026-08-18 01:51:59 -04:00)*

`crates/windows-threadpool-sys/src/wait.rs` carried `/// }, None)?;///` -- a doc-comment marker appended to the
end of a code line inside a doc example, spliced in by an earlier edit of mine and unnoticed for four rounds.

The review reported it as a compile failure. Verified against a scratch file: it is only an `unused_doc_comment`
warning and the doctest compiles and passes, and `RUSTDOCFLAGS=-D warnings` does not catch it either. The damage
was real; the stated consequence was not.

[tools/check-encoding.ps1](tools/check-encoding.ps1) now rejects `///` after a non-space character at end of
line in `.rs` files, verified to have zero false positives across the repository. The first version of the guard
was a **no-op** -- it gated on a variable not in scope inside the file loop -- which was caught only by planting
the defect. Recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md).

### <a id="le-2"></a>LE-2 — Refuse a thread minimum and maximum that contradict each other. *(completed 2026-08-18 01:51:59 -04:00)*

Self-found while chasing a test failing about 1 run in 30. `the_maximum_takes_precedence_over_the_minimum`
asserted a rule generalised from a single measurement of one ordering, and the rule is false.

Measured: `set_max_threads(2)` then `set_min_threads(4)` peaks at **4** concurrent callbacks in every one of 60
trials and does not settle back -- the minimum annuls the lower maximum silently. The reverse ordering holds at
2 in steady state but was observed peaking at 3 in 1 trial of 240 when many pools were created at once, so the
maximum is a steady-state target rather than an instantaneous ceiling.

Owner decision: track the limits the wrapper has set and reject a conflicting pair with `InvalidInput`, rather
than documenting the silent override or clamping. Each limit is tracked as an `Option<u32>` because Win32 has no
getter, so a limit we were never told cannot constrain its counterpart. Refusing the pair also makes the
overshoot window unreachable through the safe API, which removed the flake at its root rather than by loosening
the assertion. The superseded claim is marked in [DESIGN-NOTES.md](DESIGN-NOTES.md) and the new decision recorded
beside it.

## Moved 2026-08-27 -- windows-impersonation-token-sys scaffold

### <a id="it-1"></a>IT-1 -- Scaffold and register the publishable `windows-impersonation-token-sys` workspace crate. *(completed 2026-08-27 16:10:13 UTC-04:00)*

Scaffold `crates/windows-impersonation-token-sys` as a publishable Windows-only
workspace crate. Its manifest inherits the workspace authors, edition, Rust version,
license, repository, and homepage; declares version `0.1.0` with the
release-please marker; provides crates.io description, README, documentation URL,
keywords, categories, and Windows docs.rs target metadata; and selects only the
`windows-sys` foundation, security, and threading features. The crate is registered
in the workspace, [release-please-config.json](release-please-config.json),
[.release-please-manifest.json](.release-please-manifest.json), and every crate-name
surface in
[.github/workflows/publish-crate.yml](.github/workflows/publish-crate.yml),
including tag triggers, manual dispatch, and sibling-dependency recognition. Its
local Tier 1 and Tier 2 design records, plans, completed-plans, changelog, README,
manifest, and source skeleton are present with required copyright headers.

Cargo metadata discovers the package as version `0.1.0`, the targeted package check
passes, the package's empty unit and documentation test harnesses pass, and the
crate name was unclaimed on crates.io when the scaffold was completed.

## Moved 2026-08-27 -- captured impersonation token

### <a id="it-2"></a>IT-2 -- Implement the opaque, owned, clonable `ImpersonationToken` capture type. *(completed 2026-08-27 16:23:53 UTC-04:00)*

The public `ImpersonationToken::capture` operation synchronously opens the calling
thread's effective real token with `OpenAsSelf`, explicitly falls back to the
process token when the thread has none, and duplicates that context into a
non-inheritable `TokenImpersonation` handle with only `TOKEN_IMPERSONATE` access.
Existing identification, impersonation, and delegation levels are preserved;
process context becomes `SecurityImpersonation`; and anonymous context plus every
native failure stage is reported through `CaptureError` and `CaptureFailure`.

The captured handle is private, owned by `OwnedHandle`, and shared across clones
through `Arc`, so source-handle lifetime, pseudo-handle transport, mutation rights,
and rights expansion cannot invalidate the captured-context invariant. The exact
mechanics and rationale are recorded in the crate
[DESIGN-NOTES.md](crates/windows-impersonation-token-sys/DESIGN-NOTES.md) and
[DESIGN-RATIONALE.md](crates/windows-impersonation-token-sys/DESIGN-RATIONALE.md).
The targeted all-target check and Clippy pass without warnings, and the package
test and documentation-test harnesses pass.

## Moved 2026-08-27 -- impersonation token test matrix

### <a id="it-4"></a>IT-4 -- Add deterministic capture, application, restoration, and failure-path tests. *(completed 2026-08-27 17:07:20 UTC-04:00)*

The sibling unit module
[src/tests.rs](crates/windows-impersonation-token-sys/src/tests.rs) contains nine
strictly in-memory tests for capture-error classification, application failure,
closure success and error propagation, unwind drop behavior, positive
`ImpersonationToken` traits, and compile-time proof that the private application
guard is neither `Send` nor `Sync`. These tests finish in 0.00 seconds.

The real-Windows
[tests/impersonation.rs](crates/windows-impersonation-token-sys/tests/impersonation.rs)
target contains fourteen tests covering no thread token, impersonated capture,
cross-thread transport, repeated reuse, nested scopes, exact prior-token-object
restoration, closure success, closure error, unwind restoration, source-handle
lifetime independence, concurrent use, identification-level preservation,
delegation-level preservation, and anonymous rejection. Exact restoration is
verified with the prior token's `TOKEN_STATISTICS.TokenId`, so clearing to process
identity or substituting a duplicate cannot pass.

The
[tests/restoration_failure.rs](crates/windows-impersonation-token-sys/tests/restoration_failure.rs)
target verifies that restoration failure panics with the native error and that a
restoration panic during existing unwind aborts in a bounded child process. Both
tests call the same production panic helper in
[src/restore.rs](crates/windows-impersonation-token-sys/src/restore.rs).

All 25 tests pass: nine unit tests in 0.00 seconds, fourteen real-token tests in
0.00 seconds, and two restoration subprocess tests in 1.69 seconds. Targeted
all-target Clippy and documentation tests also pass without warnings.

## Moved 2026-08-27 -- scoped impersonation application

### <a id="it-3"></a>IT-3 -- Implement scoped application of an `ImpersonationToken` with exact prior-token restoration. *(completed 2026-08-27 16:41:44 UTC-04:00)*

`ImpersonationToken::with_impersonation` opens a `TOKEN_IMPERSONATE` handle to
the exact thread token present at scope entry, or records explicit no-token
process context. It applies the captured token with `SetThreadToken`, runs the
closure without interpreting its return value, and restores the saved state
before ordinary return and during unwind. Restoration reuses the same opened
token handle; it does not duplicate or normalize the token and does not call
`RevertToSelf`. A null token is used only when entry had no thread token.

The private application guard carries an `Rc` marker so it is `!Send` and
`!Sync`, and the closure-only public API prevents safe callers from forgetting
it. If `SetThreadToken` cannot restore the saved state, the guard's `Drop`
panics; restoration failure during an existing unwind triggers Rust's
double-panic abort behavior. Application failures before the closure are
reported synchronously through `ApplyError` and `ApplyFailure`.

The exact mechanics and rationale are recorded in the workspace
[DESIGN-NOTES.md](DESIGN-NOTES.md) and
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), and in the crate
[DESIGN-NOTES.md](crates/windows-impersonation-token-sys/DESIGN-NOTES.md) and
[DESIGN-RATIONALE.md](crates/windows-impersonation-token-sys/DESIGN-RATIONALE.md).
The targeted all-target check and Clippy pass without warnings, and the package
test and documentation-test harnesses pass.

## Moved 2026-08-27 -- impersonation token documentation and publication readiness

### <a id="it-5"></a>IT-5 -- Complete documentation and publication validation for the reusable impersonation-token layer. *(completed 2026-08-27 17:15:19 UTC-04:00)*

The crate-level documentation and
[README.md](crates/windows-impersonation-token-sys/README.md) now state the
capture contract, nested-result behavior, cross-thread use, owned-handle and
rights guarantees, exact prior-token restoration, and restoration-failure panic
policy. The public surface is guarded by the `missing_docs` lint, the README
contains ordinary and cross-thread examples, and
[CHANGELOG.md](crates/windows-impersonation-token-sys/CHANGELOG.md) has the
release-please baseline.

The package manifest retains the Windows docs.rs target, complete crates.io
metadata, version `0.1.0`, and only `windows-sys 0.61.2` with the Foundation,
Security, and Threading features. The crate is registered consistently in
[release-please-config.json](release-please-config.json),
[.release-please-manifest.json](.release-please-manifest.json), and
[publish-crate.yml](.github/workflows/publish-crate.yml).

`cargo publish --dry-run` packages and verifies all 15 expected files as a
55.5 KiB crate (14.0 KiB compressed). The 25 unit and integration tests and the
crate doctest pass, rustdoc and targeted all-target Clippy are warning-free, and
the default workspace passes all-target checks in debug and release modes.

> **-> CROSS-COMPONENT HANDOFF:** next work is in component
> `crates/windows-file-enumeration-sys` -> M5 -> **FE-1** (publishable enumeration
> crate scaffold). See [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-08-27 -- windows-file-enumeration-sys scaffold

### <a id="fe-1"></a>FE-1 -- Scaffold and register the publishable `windows-file-enumeration-sys` workspace crate. *(completed 2026-08-27 17:23:34 UTC-04:00)*

The new Windows-only
[Cargo.toml](crates/windows-file-enumeration-sys/Cargo.toml) inherits the
workspace authors, edition, Rust version, license, repository, and homepage;
declares version `0.1.0` with the release-please marker; and provides complete
crates.io metadata plus a Windows docs.rs target. Its path-plus-version
dependencies are `windows-impersonation-token-sys 0.1.0`,
`windows-threadpool-sys 0.1.2`, and `wtf-string 0.1.0`. Its direct
`windows-sys 0.61.2` dependency enables only Foundation, Storage FileSystem,
and System Threading for directory enumeration and CQ event signaling.

The workspace manifest and lockfile include the crate. Release automation
recognizes its component and `0.1.0` baseline, and
[publish-crate.yml](.github/workflows/publish-crate.yml) accepts its tags,
manual selection, and sibling-dependency ordering. The `file-enumeration`
Conventional Commit scope is recorded in
[copilot-instructions.md](.github/copilot-instructions.md).

The crate has its copyright-bearing library scaffold,
[README.md](crates/windows-file-enumeration-sys/README.md),
[CHANGELOG.md](crates/windows-file-enumeration-sys/CHANGELOG.md), local
[PLANS.md](crates/windows-file-enumeration-sys/PLANS.md) and
[COMPLETED-PLANS.md](crates/windows-file-enumeration-sys/COMPLETED-PLANS.md),
and Tier 1/Tier 2
[DESIGN-NOTES.md](crates/windows-file-enumeration-sys/DESIGN-NOTES.md) and
[DESIGN-RATIONALE.md](crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md).
The local design record mirrors settled workspace decisions while explicitly
leaving FE-2's public-contract questions unresolved.

The package all-target check, test harness, documentation tests, rustdoc, and
Clippy pass without warnings.

> **CROSS-COMPONENT PREREQUISITE SATISFIED:** component
> `crates/windows-impersonation-token-sys` -> M4 -> **IT-5** completed before
> this scaffold. See [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-08-27 -- file-enumeration v1 public contract

### <a id="fe-2"></a>FE-2 -- Close and record the remaining v1 public-contract decisions before implementing them. *(completed 2026-08-27 17:39:09 UTC-04:00)*

FE-2 settles caller-time ordinary-path snapshotting and explicit `\\?\`
long-path handling, native unspecified ordering, the two-record CQ and embedded
failed terminal, always-present defined inline metadata, native Windows
timestamps, selected volume qualification, the extensible query-by-example
predicate, synchronous versus accepted error boundaries, typed unsupported-
capability behavior, and the fixed aligned buffer's typed oversize-record
outcome.

The authoritative contract is in the enumeration crate's
[DESIGN-NOTES.md](crates/windows-file-enumeration-sys/DESIGN-NOTES.md), with
alternatives and constraints in
[DESIGN-RATIONALE.md](crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md).
The cross-component summary is in the workspace
[DESIGN-NOTES.md](DESIGN-NOTES.md) and
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Globazog replacement remains a
mandatory publication gate: its native metadata and predicate capability must
remain obtainable without per-entry opens.

## Moved 2026-08-27 -- file-enumeration public value types

### <a id="fe-3"></a>FE-3 -- Implement the public request, predicate, result, error, terminal, and `EnumerationId` types. *(completed 2026-08-27 18:18:50 UTC-04:00)*

The crate now carries the settled v1 value surface.
[request.rs](crates/windows-file-enumeration-sys/src/request.rs) owns
`EnumerationRequest`, whose path is validated and resolved at construction by
[path.rs](crates/windows-file-enumeration-sys/src/path.rs): `\\?\` inputs are
checked for full qualification and kept verbatim, every other form (including
`\\.\`) is resolved through `GetFullPathNameW` and held to `MAX_PATH` so
behaviour does not depend on the host's `longPathAware` manifest. Buffer
capacity defaults to 64 KiB, clamps up to 1 KiB, rounds to the 8-byte record
alignment, and rejects a value that cannot reach Win32 as a `u32`.

[predicate.rs](crates/windows-file-enumeration-sys/src/predicate.rs) and
[pattern.rs](crates/windows-file-enumeration-sys/src/pattern.rs) implement the
data-only query-by-example predicate: a non-exhaustive `EntryPredicate` seam
over a validating `QueryByExample`, ten clause forms, six comparison operators,
and a compiled single-segment name matcher whose insensitive comparison uses
`CompareStringOrdinal`. Vacuous clauses -- a zero attribute mask, an empty name
set -- are rejected when the query is built.

[entry.rs](crates/windows-file-enumeration-sys/src/entry.rs) and
[timestamp.rs](crates/windows-file-enumeration-sys/src/timestamp.rs) hold every
inline record field in native units, suppress a reparse tag the attributes do
not justify, and keep times as signed Windows ticks with `FILETIME` interop.
[error.rs](crates/windows-file-enumeration-sys/src/error.rs) splits synchronous
build failures from accepted-enumeration failures and retains every raw Win32
code, and
[completion.rs](crates/windows-file-enumeration-sys/src/completion.rs) defines
the two-record completion surface with the failure carried inside its terminal.

118 unit tests plus the crate doctest pass; targeted all-target Clippy and
`cargo fmt --check` are clean.

## Moved 2026-08-27 -- file-enumeration two-ring session and admission

FE-4 and FE-5 land in one commit. They are not independent as written: FE-4's
rings, reservations, registry, and servicer have no reachable producer until
FE-5's admission path exists, so FE-4 alone cannot build without pervasive
dead-code suppression that FE-5 would immediately remove. Committing them
together is the acknowledged-coupling response rather than disguising it with
temporary lint allowances.

### <a id="fe-4"></a>FE-4 -- Implement the bounded two-ring session shell with its `Session`, submission, and receiver types. *(completed 2026-08-27 18:49:34 UTC-04:00)*

[completion_ring.rs](crates/windows-file-enumeration-sys/src/completion_ring.rs)
is the bounded single-receiver ring: reserved terminal slots that never consume
the last data slot, best-effort entry sends that hand a refused record straight
back rather than dropping it, and a lazily created manual-reset doorbell whose
signalled state is re-established under the ring lock at the end of every
mutation.
[submission_ring.rs](crates/windows-file-enumeration-sys/src/submission_ring.rs)
is the bounded multi-producer control ring, with reserved cancellation and
abandon slots and the coalescing drain flag that keeps a burst of submissions
from queueing a burst of empty drains.

[session.rs](crates/windows-file-enumeration-sys/src/session.rs) owns both rings
plus the [registry.rs](crates/windows-file-enumeration-sys/src/registry.rs)
of live enumerations, and drains the submission ring in FIFO order from one
`ThreadpoolWork` callback. The work object is deliberately owned by the
client-side handles rather than by the state its callback touches, so a callback
can never drop the work object it is running inside.

### <a id="fe-5"></a>FE-5 -- Implement begin and cancellation admission and the affine enumeration handle. *(completed 2026-08-27 18:49:34 UTC-04:00)*

[admission.rs](crates/windows-file-enumeration-sys/src/admission.rs) secures the
captured security context, the completion-ring terminal slot, and the
submission-ring cancellation slot before a begin becomes visible, so a begin is
either fully accepted or fully refused with the request and token handed back.
`Session::try_begin` captures the submitter's context synchronously;
`try_begin_with_token` takes an already-captured one for a traversal layer.
`EnumerationHandle` is affine: cancelling or dropping it spends the reservation
exactly once, and `detach` returns it so an enumeration can outlive its handle.
Dropping the `Receiver` spends the standing abandon reservation, which stops
further starts and releases every carried enumeration without a terminal.

199 unit tests plus the crate doctest pass; targeted all-target Clippy and
`cargo fmt --check` are clean.

## Moved 2026-08-27 -- file-enumeration session model tests (M5 complete)

This group completes M5. The FE-1 through FE-5 stubs migrate with it; their
archived entries above remain the record.

### <a id="fe-6"></a>FE-6 -- Build a deterministic state-machine/model test suite for the two rings and the session. *(completed 2026-08-27 19:01:30 UTC-04:00)*

[model.rs](crates/windows-file-enumeration-sys/src/model.rs) applies scripted
operation sequences to a real session and re-checks every invariant after each
step: ring accounting stays within the bound, reservations never take the last
slot, the doorbell agrees exactly with what the receiver can observe, each
enumeration's delivered entries are an in-order prefix of what was offered, no
entry follows a terminal, and no enumeration terminates twice.

Determinism required one addition: admission rings the servicer's doorbell, so
the thread pool would otherwise race every scripted step. The model suppresses
the ring and drains through the same code path the callback uses, while the
thread-pool path keeps its own eventual-consistency tests. Modelling the engine
also required the shell-side quantum transitions (`enter_quantum`,
`leave_quantum`, `complete`) that the native engine will drive in M6.

[model/tests.rs](crates/windows-file-enumeration-sys/src/model/tests.rs) covers
twenty interleavings, including completion, two enumerations sharing a session,
quiescent cancellation, handle drop, cancellation during a running quantum
(the terminal lands behind that quantum's entries), a quantum scheduled after
cancellation, cancellation of an unknown enumeration, backpressure with retry,
backpressure shared across enumerations, parking and resumption, a terminal
delivered into a ring with no ordinary room, abandonment with and without a
running quantum, cancelling an already-completed enumeration, detachment,
session drop with an owed terminal, both minimum bounds, the completion ring's
reservation boundary, rejection of a completion ring of one, repeated cycles,
and redundant servicing and draining.

221 unit tests plus the crate doctest pass; targeted all-target Clippy and
`cargo fmt --check` are clean.

## Moved 2026-08-27 -- file-enumeration worker/servicer split

### <a id="fe-7"></a>FE-7 -- Make the worker a reporter and the servicer the sole registry authority, and give the session something to run work on. *(completed 2026-08-27 19:56:15 UTC-04:00)*

The session now owns two thread-pool objects rather than one: a servicer that
stays responsive, and an engine created with a runs-long callback environment
because any quantum may block on a directory query.
[session.rs](crates/windows-file-enumeration-sys/src/session.rs) holds both in
the shared state and hands them to the *last client handle* to release on its
own thread, so no callback can ever hold the last reference to something whose
release would wait on that callback.

`EnumerationState::work` is gone. Runnability lives in a ready set inside
[registry.rs](crates/windows-file-enumeration-sys/src/registry.rs), and
`claim_next` is single-flight: an enumeration already held is skipped rather
than run twice over the same buffer and cursor. Scheduling is idempotent and
never queues a claimed enumeration underneath its worker.

A worker now delivers its own terminal into the slot it reserved and reports
retirement through the new `Retire` control message, which every accepted
enumeration claims at admission alongside its cancellation -- raising
`MINIMUM_SUBMISSION_CAPACITY` to four. Only the servicer removes an entry, and
it returns any unspent cancellation or retirement reservation rather than
leaking it. Abandonment now releases entries that own no thread-pool object, so
receiver-drop teardown never waits on a directory query.

The state-machine model gained `Claim`, `Report`, `RunEngine`, and `Schedule`
operations plus twelve scenarios for the new control path: worker-reports-then-
servicer-retires, a retire serviced after abandonment, a report whose
enumeration is already gone, single-flight claiming, idempotent scheduling, a
finished quantum outranking a concurrent cancellation, failed and cancelled
quantum outcomes, park-and-resume through the ready set, and the minimum ring
covering both reserved control messages.

Three pre-existing tests were racing the live thread pool for work they also
drove explicitly; they now use suppressed sessions, and the tests whose subject
*is* the pool use live ones. 234 unit tests plus the crate doctest pass, stable
across fifteen consecutive runs; targeted all-target Clippy and
`cargo fmt --check` are clean.

## Moved 2026-08-27 -- file-enumeration native open and first read

### <a id="fe-8"></a>FE-8 -- Allocate the fixed native buffer and get one directory open and reading. *(completed 2026-08-27 20:12:34 UTC-04:00)*

[buffer.rs](crates/windows-file-enumeration-sys/src/buffer.rs) allocates the
staging buffer at admission, fallibly and as `u64` words so its base address is
8-byte aligned by construction rather than by hope; the ordinary growable-vector
path would abort the process on failure and guarantee only byte alignment.
`BeginFailure::BufferAllocation` reports it, and the buffer travels with the
engine state rather than the request, per D-19.

[native.rs](crates/windows-file-enumeration-sys/src/native.rs) holds the three
documented Win32 calls: open under the submitted token with
`FILE_FLAG_BACKUP_SEMANTICS`, the optional `FileIdInfo` volume query, and the
`FileIdExtdDirectoryRestartInfo` / `FileIdExtdDirectoryInfo` refill. Only the
open runs impersonated; the sibling crate's guard restores the worker's exact
prior token on every path including failure and unwind.
[engine.rs](crates/windows-file-enumeration-sys/src/engine.rs) sequences those
into a quantum that leaves the registry while it runs, so a blocking directory
query never holds the session's lock.

Two contract corrections came from the filesystem itself. A file opens
successfully with `FILE_LIST_DIRECTORY` -- it is the same bit as
`FILE_READ_DATA` -- so directory-ness is now established at the open via
`FILE_ATTRIBUTE_DIRECTORY` and reported as `DirectoryOpen(ERROR_DIRECTORY)`;
left to the first refill it would have surfaced through codes indistinguishable
from an unsupported filesystem. And an empty *subdirectory* still contains `.`
and `..`, so it returns a batch and exhausts on its second query: the
first-query-empty rule is correct but reachable only where a directory has no
records at all. Both are recorded in
[DESIGN-NOTES.md](crates/windows-file-enumeration-sys/DESIGN-NOTES.md) and
[DESIGN-RATIONALE.md](crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md).

That second correction also required `QuantumOutcome::Yielded`, one item ahead
of FE-10 which specifies it: without a way to say "one refill done, ask me
again", no directory holding records could reach its end. FE-9 replaces the
current read-and-pass-over with real parsing.

269 unit tests plus the crate doctest pass, stable across ten consecutive runs
and leaving no scratch directories behind; targeted all-target Clippy and
`cargo fmt --check` are clean.

## Moved 2026-08-27 -- file-enumeration record parsing

### <a id="fe-9"></a>FE-9 -- Parse what the buffer returns and deliver entries. *(completed 2026-08-27 20:42:04 UTC-04:00)*

[record.rs](crates/windows-file-enumeration-sys/src/record.rs) is a new module
that walks a `FILE_ID_EXTD_DIR_INFO` chain over the batch buffer, validating
alignment, fixed-field extent, next-entry-offset advance -- which now also
rejects an offset that lands inside the current record's own extent, not just
one past the batch -- name byte-length parity, name bounds, and size sign
before any field is trusted. Every field is read once, from a byte slice via
`from_ne_bytes`/`as_chunks`, never a pointer cast: later records in a batch are
only ever known to be 8-byte aligned as a whole, not individually.

[engine.rs](crates/windows-file-enumeration-sys/src/engine.rs)'s quantum now
refills at most once and then parses the loaded batch in the same quantum,
tracking a `cursor: Option<usize>` that survives across quanta exactly as D-3
specifies. `.` and `..` are dropped before the predicate ever sees them; a
match is offered to the completion ring via `try_send_entry`. A refusal parks
the quantum with the cursor left at the unparsed record -- not past it -- so
the next quantum re-parses and re-offers exactly what could not be delivered,
losing nothing. `QuantumOutcome::Idle` is now the only variant the native
engine never produces itself; its `dead_code` allow moved from the whole enum
to that one variant.

A pre-existing live-session test enumerated `C:\Windows` and counted every
completion as a terminal; now that quanta really do deliver entries, a
cancellation racing a worker could let real entries interleave with the three
expected terminals, occasionally letting the count of "3" never appear on a
run with parallel load. It now targets an empty scratch directory, which can
never produce an entry regardless of that race, matching what the test is
actually about.

290 unit tests plus the crate doctest pass, stable across fifteen consecutive
runs; targeted all-target Clippy and `cargo fmt --check` are clean.

## Moved 2026-08-27 -- file-enumeration quantum budgets

### <a id="fe-10"></a>FE-10 -- Bound each quantum and make backpressure lossless. *(completed 2026-08-27 21:32:24 UTC-04:00)*

[engine.rs](crates/windows-file-enumeration-sys/src/engine.rs)'s quantum now
bounds its own progress with two independent budgets, checked every record:
`MAX_RECORDS_PER_QUANTUM` (256) and `MAX_QUANTUM_DURATION` (2ms, a plain
monotonic `Instant`). A dropped `.`/`..`, a predicate reject, and a delivered
entry all count the same against the record budget, so a predicate that
matches nothing still yields back to the scheduler instead of running an
enormous batch to its end in one callback. Neither budget can stall an
enumeration completely: a quantum's first record is never gated by either one.
Both budgets are pure functions of `(examined, elapsed)`, unit tested directly
with synthetic values rather than real sleeping.

Completion-ring backpressure remains a separate concern, refined rather than
replaced: `EngineState::awaiting_room` remembers that the record retained at
the cursor is already known to need delivery, so a quantum resuming into a
still-full ring asks `CompletionRing::has_data_room` -- one cheap call -- and
parks again immediately rather than reparsing, rebuilding, and re-evaluating a
predicate against a record whose fate is already decided.

Recorded as D-20 in [DESIGN-NOTES.md](crates/windows-file-enumeration-sys/DESIGN-NOTES.md)
and [DESIGN-RATIONALE.md](crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md).

New tests cover: the record budget stopping a quantum mid-batch on a directory
larger than the budget, with every entry still delivered exactly once across
the quanta that follow; a directory needing several physical refills
delivering every entry once; and a still-full ring parking again on a second
resume without losing or duplicating the pending entry.

296 unit tests plus the crate doctest pass, stable across twenty consecutive
runs and leaving no scratch directories behind; targeted all-target Clippy and
`cargo fmt --check` are clean.

## Moved 2026-08-27 -- file-enumeration failure and capability taxonomy

### <a id="fe-11"></a>FE-11 -- Complete the failure and capability taxonomy the contract settled. *(completed 2026-08-27 21:49:52 UTC-04:00)*

Most of the taxonomy this item names was already in place from FE-8/FE-9:
`classify_refill_failure` in [native.rs](crates/windows-file-enumeration-sys/src/native.rs)
already mapped `ERROR_INVALID_FUNCTION`/`ERROR_NOT_SUPPORTED`/`ERROR_INVALID_PARAMETER`
to `UnsupportedExtendedDirectoryInfo` and `ERROR_MORE_DATA`/`ERROR_INSUFFICIENT_BUFFER`/
`ERROR_BAD_LENGTH` to `RecordTooLarge`; malformed records were already reported
with their [`MalformedRecord`](crates/windows-file-enumeration-sys/src/error.rs)
detail; and "a late failure truncates rather than retracts" was already proven
by the state-machine model's `a_failed_quantum_delivers_a_failed_terminal`.

What was missing was the assertion half of the contract: the unsupported-class
mapping is safe to trust only when the crate's own preconditions -- a live
crate-opened handle, a valid information class, a non-null 8-byte-aligned
buffer base, and an effective capacity that is at least
`MINIMUM_BUFFER_CAPACITY`, an 8-byte multiple, and `u32`-representable -- all
hold, and nothing checked that. `refill` now `debug_assert`s every one of them
immediately before the call whose failure `classify_refill_failure` reads,
so a future regression in handle, class, or buffer handling would be caught as
this crate's own bug rather than silently reported as a filesystem
incapability. None are independently reachable through the crate's public
API -- the type system and `NativeBuffer::try_new`'s own assertion already
rule out every violation -- so this is a regression guard, not new externally
observable behaviour, and the full existing suite passing unchanged confirms
none of them ever fire.

A new engine-level test, `a_late_malformed_record_truncates_rather_than_retracts`,
proves the truncation property against the real parser rather than the
scripted model: it parks an enumeration on a full ring, frees exactly one slot,
corrupts the still-retained record's `NextEntryOffset` directly in the native
buffer, and confirms the resulting failure leaves the one entry still queued
untouched.

Recorded in [DESIGN-NOTES.md](crates/windows-file-enumeration-sys/DESIGN-NOTES.md)
(D-13's section, "Error taxonomy and capability failures").

297 unit tests plus the crate doctest pass, stable across twenty consecutive
runs and leaving no scratch directories behind; targeted all-target Clippy and
`cargo fmt --check` are clean.

## Moved 2026-08-27 -- file-enumeration cancellation, abandonment, and teardown

### <a id="fe-12"></a>FE-12 -- Complete cancellation, abandonment, and teardown around the live engine. *(completed 2026-08-27 22:14:30 UTC-04:00)*

The architecture this item names was already in place from FE-7 (D-16
through D-18) and already proven, abstractly, by the state-machine model:
cancellation cannot preempt a quantum in flight because `report_quantum`
only overrides its outcome (`_ if state.cancelled => Cancelled`) after the
quantum returns; a quiescent cancellation or abandonment removes and
releases a registry entry immediately, without a thread-pool object to wait
on; and a stale ready-queue id left behind by a removed entry is a
documented, harmless no-op for `claim_next` to skip. Auditing the code
found no defect in any of it.

What FE-12 adds is proof against the *real* engine -- real files, real
refills, the real completion ring -- rather than only the scripted model,
plus a repeated-cycle audit that would catch a leak the model's one-shot
scenarios could not:

- [session/tests.rs](crates/windows-file-enumeration-sys/src/session/tests.rs)
  gained `cancelling_a_yielded_real_enumeration_preserves_entries_and_ends_the_stream`
  (a real quiescent cancellation, driven deterministically with a suppressed
  pool) and `cancellation_observed_while_a_worker_holds_the_engine_is_deferred_behind_its_report`
  (the engine claimed and mid-quantum when cancellation is serviced, proving
  the quantum itself runs to its natural conclusion with no knowledge of it).
  Both assert, via a new `drain_ordered` helper, that no entry ever follows a
  terminal and that at most one terminal arrives.
- `repeated_cycles_through_every_terminal_kind_leak_no_reservation` runs
  thirty cycles of success, failure, and cancellation on a session sized to
  the bare minimum (`MINIMUM_SUBMISSION_CAPACITY`,
  `MINIMUM_COMPLETION_RING_CAPACITY`): any leaked cancel, retire, or terminal
  reservation would exhaust that room long before the thirtieth repeat.
- `abandonment_does_not_wait_on_a_directory_query` and
  `dropping_every_handle_while_a_real_enumeration_is_running_does_not_hang`
  exercise the live thread pool directly: the first times the receiver's
  drop against four real, running enumerations; the second's entire
  assertion is that dropping every handle mid-enumeration completes at all,
  which is exactly what a self-wait in the worker-reports design would
  violate.

Two test-construction bugs surfaced and were fixed, not production defects:
a live test undersized its completion ring against real entries a worker
delivers before the test ever drains them, and a worker-reports scenario
was missing the second `drain_submissions` call that applies the retire
message `report_quantum` posts but does not itself service.

302 unit tests plus the crate doctest pass, stable across thirty consecutive
runs and leaving no scratch directories behind; targeted all-target Clippy
and `cargo fmt --check` are clean. This completes M6.

## Moved 2026-08-27 -- file-enumeration real-Windows integration suite

### <a id="fe-13"></a>FE-13 -- Build the real-Windows integration suite. *(completed 2026-08-27 22:33:56 UTC-04:00)*

A new integration test crate under
[tests/integration/](crates/windows-file-enumeration-sys/tests/integration/)
(`main.rs` plus eight scenario modules, using the `tests/<name>/main.rs`
layout so `mod` declarations resolve). Built entirely on the crate's public
API, since an integration test is a separate crate and cannot reach
`src/scratch.rs` or `src/testing.rs` (both `pub(crate)`); a dedicated
`support.rs` supplies its own self-deleting `Scratch` fixture and drain
helpers, including `drain_many` for scenarios running several enumerations
concurrently on a shared receiver.

- `directories.rs` -- ten independent ordinary directories on one session,
  an empty directory, a single-entry directory, and files mixed with
  subdirectories reported with the right `EntryType`.
- `scale.rs` -- 4,000 entries in one directory (forcing multi-refill),
  a completion ring at `MINIMUM_COMPLETION_RING_CAPACITY` against 500 entries
  (forcing sustained park/resume), and `MINIMUM_BUFFER_CAPACITY` /
  `DEFAULT_BUFFER_CAPACITY` agreeing on the same directory.
- `cancellation.rs` -- cancel before any quantum runs, cancel racing a real
  4,000-entry enumeration on the live pool, cancel after completion (no
  second terminal), and receiver drop against a live running enumeration.
- `paths.rs` -- a missing directory, a file opened as a directory, a
  present-but-often-restricted system directory (accepting either outcome,
  since the test host's ACL is not this crate's to assume), a `\\?\` path
  built to exceed 260 characters, an ordinary path rejected for exceeding it,
  and a filename holding an unpaired UTF-16 surrogate built via
  `OsStringExt::from_wide`, round-tripped byte-for-byte.
- `reparse.rs` -- a real directory junction (via `mklink /J`, needing no
  elevated privilege, unlike a symlink), confirming the reparse attribute,
  `is_reparse_point()`, and `IO_REPARSE_TAG_MOUNT_POINT`.
- `predicates.rs` -- all six `ComparisonOperator` variants against known file
  sizes; both `CaseSensitivity` modes; `IsType`, `NameInSet`, a wildcard
  `AnyRun` pattern, `AttributesAllSet`/`AttributesAllClear` against a real
  read-only file, `IsReparsePoint` negated, and all four `TimestampField`
  variants.
- `metadata.rs` -- logical size cross-checked against `std::fs::metadata`,
  every inline field reachable for both a file and a directory,
  `BestEffort` identity volume-qualified on a local disk, and two real files'
  identities sharing one volume serial while carrying distinct file IDs.
- `capability.rs` -- documents, per the user's explicit decision, that
  `UnsupportedExtendedDirectoryInfo` and `RecordTooLarge` are proven only at
  the unit level (FE-11, synthetic Win32 codes): no incompatible filesystem
  or redirector is available in this environment to reach either
  organically, and `MINIMUM_BUFFER_CAPACITY` structurally rules out the
  latter through the crate's own public buffer sizing regardless. Tests that
  a below-minimum buffer clamps up rather than ever reaching it.

One test-construction issue surfaced while writing `directories.rs`: the
ten-concurrent-enumerations scenario shares one receiver across enumerations
that interleave arbitrarily, which a single-enumeration-only drain helper
cannot assume; `drain_many` was added rather than weakening
`drain_to_terminal`'s per-enumeration ordering check, which every
single-enumeration scenario still depends on.

302 crate unit tests, 31 new integration tests, and the crate doctest pass;
stable across eight consecutive integration runs (~1s each) and leaving no
scratch directories behind. Targeted all-target Clippy and
`cargo fmt --check` are clean.
