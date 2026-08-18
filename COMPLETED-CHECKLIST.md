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
