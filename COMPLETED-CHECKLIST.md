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

The `restoration_failure_panics_with_the_native_error` and
`restoration_failure_during_unwind_aborts_the_process` unit tests in
[src/tests.rs](crates/windows-impersonation-token-sys/src/tests.rs) verify
that restoration failure panics with the native error and that a
restoration panic during existing unwind aborts in a bounded child process
(re-executing the unit-test binary itself, not a separate integration test
target). Both tests call the same production panic helper in
[src/restore.rs](crates/windows-impersonation-token-sys/src/restore.rs).

All 25 tests pass: nine unit tests in 0.00 seconds, fourteen real-token tests in
0.00 seconds, and two restoration subprocess tests in 1.69 seconds. Targeted
all-target Clippy and documentation tests also pass without warnings. (The two
restoration subprocess tests were later moved from a separate integration test
target into `src/tests.rs` as ordinary unit tests, addressing a Copilot PR #44
review finding that the integration test's `#[path]` inclusion of `src/restore.rs`
bypassed the crate's real compiled module graph.)

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


## Moved 2026-08-27 -- file-enumeration Globazog adapter demonstration

### <a id="fe-14"></a>FE-14 -- Discharge the D-15 Globazog acceptance gate with a real adapter demonstration, not a metadata cross-check. *(completed 2026-08-27 22:54:47 UTC-04:00)*

A hand-reconstructed adapter under
[tests/integration/globazog_adapter/](crates/windows-file-enumeration-sys/tests/integration/globazog_adapter.rs)
reimplements Globazog's real Windows one-directory backend's public value
types and predicate vocabulary and exercises the live native engine through
it. Globazog is never an actual dependency of this workspace -- it is meant
to consume this crate, not the reverse -- so every reconstructed type carries
a doc comment citing the exact file it was copied from at `MikeGrier/globazog-rs`
commit `55a0b1aec7a93051a675852636ab41a6437440fb`
(`crates/globazog/src/{sys,sys/win,predicate,syntax,syntax/decode,error}.rs`):

- `types.rs` -- `DirEntry`, `DirScan`, `EntryFailure`, `EnumPlan`, `FileId`,
  `decode_utf16` (ported verbatim to preserve Globazog's unpaired-surrogate
  handling), its inverse `encode_codepoint_to_wtf16` (written from scratch,
  Globazog never needs that direction), and the FILETIME-to-Unix-nanos
  conversion Globazog's real backend uses.
- `predicate_types.rs` -- `CaseSensitivity`, `Token`, `Segment`, `Cmp`,
  `TimeField`, `Leaf`, with `Leaf::Depth` deliberately excluded: it is a
  property of Globazog's own recursive multi-directory traversal engine, not
  something a one-directory backend can ever be asked to answer.
- `translate.rs` -- `translate_leaf`/`translate_segment`/`translate_leaves`,
  including the `EntryType::Other` case: Windows has no third entry kind, so
  a non-negated `IsType{ty:Other}` translates to a self-contradictory
  attribute-clause pair (the directory bit required both set and clear in
  the same conjunction) rather than being silently dropped.
- `adapter.rs` -- `enumerate_dir_native_via_wfe(_with_predicate)`,
  `translate_entry`, and `finish_scan` as a pure function separated from live
  I/O specifically so the error-shape contract can be unit-tested without a
  live filesystem fault.

Two properties D-15 requires could not be reached organically in this
environment, and both are narrowed to a proof that still covers the
contract, matching the precedent FE-13's `capability.rs` set:

- A genuine live late-failure (`TerminalOutcome::Failed` arriving after some
  entries were already delivered) needs a filesystem or redirector fault this
  environment cannot manufacture on demand -- proven instead via
  `tests_errors.rs` calling `finish_scan` directly with hand-built
  `TerminalOutcome::Failed` values, both with and without prior entries.
- "No path opens an individual entry" (inherited from D-3) is proven via
  `tests_no_per_entry_open.rs`: a directory junction whose target does not
  exist is still listed successfully by the batched directory query, which
  would not hold if entries were resolved individually.

Two test-construction bugs surfaced while writing `tests_metadata.rs`, not
production defects: a `target` directory created as a junction's destination
was itself a fourth top-level listable entry alongside the three the test
expected, fixed by nesting it under a subdirectory; and passing a compound
slash-containing string (`"plain-dir/target"`) to the shared `Scratch::subdir`
helper produced a path `cmd.exe`'s own command-line re-parsing of the
`mklink /J` invocation mis-tokenized around, fixed by composing the nested
path with `PathBuf::join` instead so every component keeps native `\`
separators.

53 new integration tests pass alongside the existing 302 unit tests and the
crate doctest, stable across ten consecutive integration runs and leaving no
scratch directories or junctions behind. Targeted all-target Clippy and
`cargo fmt --check` are clean. `DESIGN-NOTES.md`'s Globazog replacement gate
section and `DESIGN-RATIONALE.md` record the discharge and both acknowledged
limitations.


## Moved 2026-08-27 -- file-enumeration API documentation and changelog baseline

### <a id="fe-15"></a>FE-15 -- Complete crate-level API and safety documentation, README examples covering ordinary and traversal-style submission, and the changelog baseline. *(completed 2026-08-27 23:03:43 UTC-04:00)*

Removed the stale M5/M6 shell caveat from `lib.rs`'s top doc comment (it said
the session and native engine were "scheduled by M5 and M6," both long since
implemented) and replaced it with a `# Safety` section stating the actual
guarantee: the public surface is entirely safe, every native call is confined
to one caller-owned size-checked buffer, no entry is ever opened
individually, and a submitted enumeration's security context is captured
synchronously on the submitter's own thread before the request becomes
visible to any worker -- with a pointer to `DESIGN-NOTES.md`/
`DESIGN-RATIONALE.md` for the unsafe internals that make it true.

Added two new doctested examples to `lib.rs` alongside the existing
predicate-building one: "Running an enumeration" (`Session::new`,
`try_begin`, draining to `Completion::Terminal`) and "Traversal-style
submission" (`ImpersonationToken::capture` once, reused via
`try_begin_with_token` across several directories instead of a fresh capture
per directory). Both compile under `cargo test --doc` (3 doctests, up from
1).

`README.md`'s "Status" section, which still named FE-3 through FE-11 as in
progress, now states the public API, session, native engine, and Globazog
adapter demonstration are complete, with only FE-16 (publication validation)
remaining. Added a matching "Examples" section mirroring both `lib.rs`
doctests for a reader who only opens the README.

`CHANGELOG.md` was empty (just a heading); gave it the same boilerplate
release-please baseline every other not-yet-released crate in this
workspace carries (`windows-impersonation-token-sys`'s, verbatim).

302 unit tests, 53 integration tests, and now 3 doctests pass. Targeted
all-target Clippy and `cargo fmt --check` are clean; `missing_docs` remains
warning-free with no new suppressions needed.


## Moved 2026-08-27 -- file-enumeration publication validation

### <a id="fe-16"></a>FE-16 -- Validate publication: packaged contents, docs.rs metadata, release automation, sibling-dependency version ordering against crates.io, and `cargo publish --dry-run`. *(completed 2026-08-27 23:30:48 UTC-04:00)*

`cargo package --list` confirms the packaged contents: 71 files -- every
git-tracked file under the crate (`Cargo.toml`, `README.md`, `CHANGELOG.md`,
`PLANS.md`/`COMPLETED-PLANS.md`, `DESIGN-NOTES.md`/`DESIGN-RATIONALE.md`,
every `src/*.rs` including the sibling `tests.rs` modules, every
`tests/integration/**/*.rs`) plus the three files Cargo generates for every
package (`.cargo_vcs_info.json`, `Cargo.lock`, `Cargo.toml.orig`). Nothing
unexpected is included or missing.

`Cargo.toml`'s `[package.metadata.docs.rs]` pins `x86_64-pc-windows-msvc` as
both the default and only target -- required because the crate is
`cfg(windows)`-only and docs.rs's default Linux target would otherwise render
an empty crate. `description`, `keywords`, `categories`, `readme`,
`documentation`, `repository`, and `homepage` are all present and accurate.

Release automation is registered consistently in
[release-please-config.json](release-please-config.json),
[.release-please-manifest.json](.release-please-manifest.json), and
[.github/workflows/publish-crate.yml](.github/workflows/publish-crate.yml)
(both the tag trigger and the `workflow_dispatch` crate list), matching every
sibling crate's entry shape exactly.

Sibling-dependency version requirements were checked against what is actually
live on crates.io: `windows-threadpool-sys = "0.1.2"` is satisfied by the
published `0.1.3`; `wtf-string = "0.1.0"` is satisfied by the published
`0.1.0`. `windows-impersonation-token-sys = "0.1.0"` is **not yet
satisfiable** -- the crate has never been published (confirmed via the
crates.io API returning 404, `gh release list` showing no
`windows-impersonation-token-sys-v*` release, and no matching tag in this
repository) -- because this entire feature branch, which introduces both
`windows-impersonation-token-sys` and `windows-file-enumeration-sys`, has not
yet merged to `main`, so release-please has never run a release cycle for
either crate. `origin/main` has independently advanced its own release cycle
in the meantime (`windows-threadpool-sys` to `0.1.3`, `windows-overlapped-io-sys`
to `0.1.3`, `windows-ioring-sys` to `0.1.2`, `windows-file-watcher` to `0.1.2`),
which is expected and does not affect this crate's dependency requirements.

`cargo publish --dry-run` therefore fails at the dependency-resolution step
with `no matching package named windows-impersonation-token-sys found`. This
was confirmed to be exactly that -- and not a packaging defect -- by cross-
checking against `cargo package --list` (which succeeds with the full,
correct 71-file list above) and by inspecting the partial archive cargo
leaves behind in `target/package/tmp-crate/` when the dependency-resolution
step aborts mid-write: an incomplete 3-file fragment, not a real content gap.
This is the user-acknowledged, explicitly recorded blocker for this item: a
full `cargo publish --dry-run` cannot go green until this branch merges to
`main` and a release-please release ships `windows-impersonation-token-sys`
to crates.io first. `publish-crate.yml`'s existing "wait for workspace-sibling
dependencies on crates.io" step already makes the real publish order robust
to this exact ordering constraint (it polls the sparse index and blocks a
dependent crate's tag-triggered publish until every workspace-sibling
dependency it declares is live at the required version), so no workflow
change is needed -- only time, and the merge this branch is waiting on.

Everything else validated cleanly: the default workspace's all-target check
passes with no warnings in both debug and release, and the crate's own
suite -- 302 unit tests, 53 integration tests, and 3 doctests -- passes,
with targeted all-target Clippy and `cargo fmt --check` clean. This completes
M7, and with it the whole M1-M7 arc this checklist file tracked.

## Moved 2026-08-27 -- M6 and M7 milestone index archived

The native enumeration engine (M6) and verification/Globazog
acceptance/publication (M7) milestone headings and item indexes, relocated
from [CHECKLIST.md](CHECKLIST.md) now that every item in both is complete and
has its own detailed record above (or, for M6, in the earlier "native
enumeration engine" moved section).

### M6 -- Native enumeration engine

M5's shell left a latent hazard that M6 had to remove before it could install
a worker: `leave_quantum` and `complete` let a worker mutate the registry and
drop its own thread-pool object from inside its own callback, which
self-waits and frees the executing closure. FE-7 closed that by making the
worker a reporter and the submission-ring servicer the sole registry
authority (D-16, D-17).

- [x] **FE-7** -- Make the worker a reporter and the servicer the sole registry authority, and give the session something to run work on. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-7)
- [x] **FE-8** -- Allocate the fixed native buffer and get one directory open and reading. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-8)
- [x] **FE-9** -- Parse what the buffer returns and deliver entries. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-9)
- [x] **FE-10** -- Bound each quantum and make backpressure lossless. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-10)
- [x] **FE-11** -- Complete the failure and capability taxonomy the contract settled. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-11)
- [x] **FE-12** -- Complete cancellation, abandonment, and teardown around the live engine. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-12)

### M7 -- Verification, Globazog acceptance, and publication

- [x] **FE-13** -- Build the real-Windows integration suite. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-13)
- [x] **FE-14** -- Discharge the D-15 Globazog acceptance gate with a real adapter demonstration, not a metadata cross-check. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-14)
- [x] **FE-15** -- Complete crate-level API and safety documentation, README examples covering ordinary and traversal-style submission, and the changelog baseline. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-15)
- [x] **FE-16** -- Validate publication: packaged contents, docs.rs metadata, release automation, sibling-dependency version ordering against crates.io, and `cargo publish --dry-run`. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-16)

## <a id="moved-2026-08-27-m1"></a>Moved 2026-08-27 -- M1: amplify PR #42's contract-specification findings across the delivery-contract crates

PR #42 ("Testability: consumer test surface for windows-file-watcher + example test harness crate") took
**19 automated review rounds**, and the review-response phase (39 commits, 2,077 insertions) added more code
than the original implementation (16 commits, 3,220 insertions) did. The dominant failure was not
implementation error: `windows-file-watcher`'s delivery contract, written as prose, was **true but
incomplete** in categorizable ways, and the gaps stayed invisible until a second implementation (the example
harness's contract-legal generator, its own D-5) had to obey the contract mechanically. Eight rounds fixed
generated sequences the watcher could never emit; five corrected the contract prose itself; one found a real
shipped reliability defect (`has_room`, 700e0eb) sitting on D-29's backpressure path.

The transferable asset was the **taxonomy of gap categories**, not the individual fixes.

- [x] **M1.1** -- Recorded the ten gap categories in the workspace
  [DESIGN-NOTES.md](DESIGN-NOTES.md#specifying-a-delivery-contract), each pinned to the PR #42 commit that
  evidences it, plus the same-author hazard, why a passing test suite cannot surface them (they are
  statements about the *set* of legal sequences, not points in it), and the `has_room` finding as evidence
  that the cost is real rather than editorial. Canonical home; per-crate notes reference rather than restate.

- [x] **M1.2** -- `windows-file-watcher`: recorded D-84, naming which decisions were stated incompletely and
  how (D-9, D-12/D-30, D-17, D-27/D-28, D-50/D-78, D-83), and the `has_room` finding separately as a defect
  in shipped 0.1 code rather than a harness bug. Queued the audit it does *not* claim to have done as that
  crate's M14.

- [x] **M1.3** -- `windows-overlapped-io-sys`: found two categories it had already paid for before the
  taxonomy named them (`Issued`'s state-dependent legality, which hung rundown until M10.5; `post`/`post_raw`'s
  arbitrary completion key), two it got right (`OperationId` generations, removing `from_parts`), and one
  consequential omission -- **completion observation order was never stated**, which matters because
  `windows-file-watcher` builds on this crate and *does* promise ordering to its own clients. Remaining
  categories queued as that crate's M14.

- [x] **M1.4** -- `windows-ioring-sys`: cited D-17 (`RingId`) and `Completion::synthetic`'s test-only gate as
  the pattern done right, recorded D-14 as an honestly-flagged cross-message continuity assumption, and
  stated the previously-missing completion-ordering rule -- a gap this crate is *more* exposed to than its
  siblings, since "ring" invites the ordered-queue assumption and `drain_preceding`'s existence was the only
  available evidence. Remaining categories queued as that crate's M10.

Docs-only: nine `.md` files, no `.rs` touched. The audits are deliberately partial -- five of ten categories
reached in overlapped-io, four of ten in ioring -- with the rest recorded as "not examined" rather than "does
not apply", because that distinction is the point of having the taxonomy. Completing them is each crate's own
milestone: [windows-file-watcher M14](crates/windows-file-watcher/CHECKLIST.md),
[windows-overlapped-io-sys M14](crates/windows-overlapped-io-sys/CHECKLIST.md), and
[windows-ioring-sys M10](crates/windows-ioring-sys/CHECKLIST.md).

## <a id="moved-2026-08-27-m2"></a>Moved 2026-08-27 -- M2: stop contract corrections from failing to propagate

[M1](#moved-2026-08-27-m1) recorded the ten specification-gap categories, which address **under-specification**
-- what a contract fails to say. Executing it exposed a second failure mode the taxonomy has no mechanism
for: **restatement drift**, where one fact is stated in several independent places, a correction reaches some
of them, and the rest keep teaching the old answer. Across three consecutive PR #42 review rounds, five of six
findings were corrections that had not propagated rather than original defects. Recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md#restatement-drift).

- [x] **M2.1** -- Compiled `windows-file-watcher`'s
  [TESTING.md](crates/windows-file-watcher/TESTING.md) and
  [README.md](crates/windows-file-watcher/README.md) as doctests. Neither was
  compiled before -- there was no `include_str!` anywhere -- so the five Rust blocks across them could only
  rot, and one was among the four sites that taught the `Stopped` error. Doctest count went 2 -> 7. Verified
  by reintroducing the exact drift and confirming the failure (`left: 2, right: 1`) before reverting; CI's
  `cargo test --workspace --all-features` covers `test-util`, so the guard is live there rather than local
  only.

- [x] **M2.2** -- `DesyncCause::is_terminal()`, adopted at all four example sites. The terminal-vs-recoverable
  distinction was restated across 8 files and drifted in 4 at once. Asking the cause rather than matching
  `Stopped` by name also keeps a handler correct if a further terminal cause is added, where a name-match
  would silently treat it as recoverable and re-scan a dead watch forever.

- [x] **M2.3** -- `DesyncCause::is_reachable_in(WatchMode)`, with the harness generator binding to it rather
  than re-encoding tier legality. That fact had four independent encodings and drifted in *both* directions
  across two rounds. Verified the binding is real by sabotage: changing the crate's definition changed the
  generator's **output**, not merely a test's expectation. One test written during this item was deleted
  rather than shipped -- it compared `is_reachable_in` against `to_cause().is_reachable_in()`, which is the
  same expression, so it was tautological and redundant with the existing mirror test.

- [x] **M2.4** -- Recorded [Restatement drift](DESIGN-NOTES.md#restatement-drift) with the measurement rather
  than the impression (13 files restate `QueueFull`, 8 restate "`Stopped` is terminal"), why the taxonomy
  cannot catch it, and the three-tier remedy. Cross-linked from the taxonomy section so a reader arriving at
  the ten categories learns that stating a rule correctly is necessary and not sufficient.

- [x] **M2.5** -- Added a `CONTRACT INTEGRITY` section to
  [.github/copilot-instructions.md](.github/copilot-instructions.md): prefer a derived fact to a restated one
  (verified by sabotage), prose that contains code must compile, and a mandatory blast-radius sweep before
  any contract correction -- with the two corollaries that each already cost a review round, that an analysis
  document never restates normative content, and that correcting a shipped rule obliges re-checking whatever
  was built against the old one.

Net effect: the two facts that actually drifted are now derived rather than restated, the prose that taught
them is compiled, and what neither mechanism can reach is a binding rule in the file humans and Copilot both
read. 943 workspace tests pass; default workspace builds clean in debug and release.

## <a id="moved-2026-08-27-m3"></a>Moved 2026-08-27 -- M3: make the sequencing rules executable too

[M2](#moved-2026-08-27-m2) made the *value-level* contract facts derived rather than restated, and recorded
that sequencing rules would stay prose. That was too pessimistic: what cannot express them is the **type
system**, not the codebase. A shared executable oracle can, and it is the same derive-don't-restate move at
runtime.

- [x] **M3.1** -- Renamed `first_two_notifications_of_a_liveness_watch_are_established_then_subscribed`,
  which asserted as a *contract* rule something M14.2 had already established is not universally true: a
  route coalescing onto an already-faulted watcher sees `Completion { Subscribed }` first and its
  `Established` only after recovery. It passed solely because the generator never produces that case -- a
  generator property wearing a contract name, and drift that had already happened with nothing catching it.

- [x] **M3.2** -- Added `ContractChecker` to `windows-file-watcher` behind `test-util`: a per-`WatchId` state
  machine checking terminality, tier-conditioned emission (delegated to `DesyncCause::is_reachable_in`), and
  D-50/D-78 volume continuity and distinctness. It lives in the crate, not the harness, so one definition
  serves the crate's tests, the harness, and a consumer's test doubles. Equal care went into what it does
  **not** check: six tests assert it *accepts* the sequences M14 found legal but surprising, since
  over-constraining is the same defect as under-specifying and this crate has shipped it. The one rule
  genuinely uncheckable from the stream -- at most one question outstanding, whose answer travels the request
  queue -- is documented as such rather than approximated.

- [x] **M3.3** -- Adopted the checker in `windows-file-watcher`'s own integration tests, at the single
  `Drained::pump` funnel every test drains through, so the **real** watcher's output is validated rather than
  spot-checked. No violations found. Verified the guard actually fires rather than being compiled out by its
  feature gate: sabotaging the checker made 13 of 15 tests fail.

- [x] **M3.4** -- Collapsed four hand-written sequencing restatements in the harness into one
  "generate, then validate" test. Two tests were kept and renamed to say whose property they assert:
  generator *coverage* (which a contract checker cannot supply -- it says nothing illegal was emitted, never
  that anything interesting was) and a deliberately narrower generator rule.

Net effect: the sequencing rules now have one executable definition that the crate's own tests, the harness
generator, and any consumer bind to. **Known remaining gap, left visible rather than papered over:** two
harness tests still hand-encode contract rules the checker does not cover (`Resumed` followed by
`Established`, and an interactive fault always asking). Extending the checker to them is real work, not
bookkeeping.

## <a id="moved-2026-08-27-m3-followup"></a>Moved 2026-08-27 -- M3 follow-up: contract checking extended to every real-watcher drain

[M3.3](#moved-2026-08-27-m3) recorded adopting `ContractChecker` "at the single `Drained::pump` funnel every
test drains through". That was true of [tests/watched_paths.rs](crates/windows-file-watcher/tests/watched_paths.rs)
and its 15 tests, but read as crate-wide coverage, which it was not: `tests/fault_detail.rs` and
`tests/stress.rs` also drain real `Monitor` sessions, through their own loops, and neither was checked
(PR #42 review).

The claim is now true rather than narrowed. Both files route every drain through the checker:
`fault_detail.rs` at its own `drain_until` funnel, and `stress.rs` at all four of its drain sites via a
`Guard` alias that compiles to a no-op when `test-util` is off, so the call sites need no `cfg`.

The stress suite is the more valuable of the two: `a_fault_storm_of_repeated_delete_recreate_always_reestablishes`
walks the fault-bracket path 25 times against a real directory being deleted and recreated, which is exactly
where a sequencing violation would appear and exactly where a point assertion would not notice. Run with
`WINDOWS_FILE_WATCHER_STRESS=1`, all four stress tests pass with the checker live and report no violations.

## <a id="moved-2026-08-27-m3-correction"></a>Moved 2026-08-27 -- M3 correction: one of the two "remaining gaps" was not a contract rule

[The M3 follow-up](#moved-2026-08-27-m3-followup) recorded two harness tests as hand-encoding "contract rules
the checker does not cover", and named extending the checker to them as real work. One of the two was not a
contract rule at all (PR #42 review).

`resolve_fault_success` issues `Resumed` and `Established` back to back, and the M3 entry read that as
"always together". But each is a **separate best-effort observation send** ([D-57](crates/windows-file-watcher/DESIGN-NOTES.md)),
so a saturated queue can take `Resumed` and latch `Established` into a `Desync { QueueFull }`. Together
describes the *attempt*, not the delivery. There is therefore no invariant to extend the checker with, and
adding one would have made it reject production output -- the same over-constraint the checker's must-accept
tests exist to prevent, this time queued as planned work.

The test is kept and renamed `this_generator_always_pairs_resumed_with_established`, which is what it
actually asserts: the generator models the unsaturated case. The same false claim was corrected at five other
sites in the same change (the schedule module's legality guide, the generator's module docs and two inline
comments, and this test's own comment).

The other remaining gap -- an interactive fault always asking -- stands as recorded.

## <a id="checklist-review-baseline"></a>Moved 2026-08-28 -- automated-reviewer language baseline (CHECKLIST-review-baseline.md, M1)

Closed the gap that let an automated PR review on
[#46](https://github.com/MikeGrier/windows-threadpool-sys/pull/46) raise seven false
"`size_of` is not in scope, this will not compile" findings against code that builds clean on
this workspace's pinned toolchain. Three things had to coincide, and two were ours: the
baseline is structurally invisible in a diff (the toolchain pin never appears, the root
manifest's `[workspace.package]` table fell six lines outside the only hunk, and the new crate
manifests carry `edition.workspace = true`, a pointer to a table in no hunk); the workspace's
own pre-1.80 `size_of` call sites supplied genuine in-repo evidence for the wrong reading; and
nearly all existing Rust predates the 1.80 prelude change. Only the third was outside our
control.

Validated empirically rather than argued. Re-running the same reviewer on the same PR after
the change took it from **7 comments generated / "changes recommended"** to **0 new comments**,
with the `size_of` claim absent; the remaining verdict was a scope observation asking for human
review of a 112-file, three-new-crate PR, which is correct.

Decisions recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md#restatement-drift) (a fourth remedy for
restatement drift, for a fact none of the previous three reach) and
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) (why the baseline is checked rather than centralised,
with the three rejected alternatives).

- [x] **RB-1** -- Create [.github/instructions/global.rust.instructions.md](.github/instructions/global.rust.instructions.md).
  [.github/copilot-instructions.md](.github/copilot-instructions.md) already cited this path
  twice (at the "Rust pre-commit gate" bullet and at the milestone-boundary build step) as the
  home of "the full gate", but the file did not exist -- so the one natural home for a Rust
  language baseline was a dangling reference. Created as the authoritative Rust document: the
  language baseline (edition, MSRV, pinned toolchain, and the consequence that 1.80+ prelude
  items are used unqualified), then the full pre-commit gate the root file summarises. Both
  existing references converted into clickable relative links. Two false claims in the root
  file were found while writing it and corrected in the new file rather than copied forward:
  there is no `.config/nextest.toml` in this repository and cargo-nextest is not installed, and
  `UNRESOLVED-TEST-FAILURES.md` is a per-component file rather than a root one.

- [x] **RB-2** -- Add a short Rust language baseline section to
  [.github/copilot-instructions.md](.github/copilot-instructions.md). That file is the one an
  automated PR reviewer is known to read, and it contained zero occurrences of `edition`,
  `MSRV`, `1.98`, `rust-version`, or `prelude` across its whole length. Placed first in the
  file, states the edition and MSRV outright (a reviewer cannot follow a link out of a diff),
  names the prelude items this unlocks, generalises to the whole 1.80 -> 1.98 window rather
  than to `size_of` alone, instructs that a compile claim be verified before it is reported,
  and points at RB-1's file for the rest.

- [x] **RB-3** -- Normalise the pre-1.80 `size_of` call sites so the workspace stops
  contradicting itself. Seven sites across three crates became the bare prelude form and four
  now-unused imports were dropped (two `use std::mem::size_of;`, two `use core::mem;`). This
  was the confirming evidence above: while it stood, a reviewer pattern-matching against
  repository precedent would keep reaching the same wrong conclusion whatever the instruction
  files said. `ManuallyDrop` and `MaybeUninit` imports untouched -- they are not in the prelude.

- [x] **RB-4** -- Guard the restated baseline against drift in CI, via
  [tools/check-baseline.ps1](tools/check-baseline.ps1) and the `language baseline consistency`
  job. **Re-planned during execution:** the item as written assumed the restatements lived in
  "either instruction file", but a blast-radius sweep found **twelve claims across six files** --
  also README.md, DEVELOPMENT.md, .github/dependabot.yml, and ci.yml's own `msrv` job name and
  toolchain pin. The check parses the two authoritative declarations
  (`[workspace.package]` in [Cargo.toml](Cargo.toml), `[toolchain]` in
  [rust-toolchain.toml](rust-toolchain.toml)), verifies they agree with each other, then makes
  two passes: each labelled claim is matched by its own regex and compared, and every
  Rust-version-shaped token in a registered file must be the MSRV, the channel, or an
  allow-listed historical version carrying a recorded reason. A claim that no longer matches
  its regex fails rather than passes, catching a reword that drops the value. Verified by
  sabotage per the rule that a binding which cannot be shown to fail is cosmetic: changing
  `rust-version`, `channel`, or `edition`, deleting a claim's value, and planting a stale
  version in prose each produce a distinct located failure; exit 2 is reserved for
  configuration errors and the script is cwd-independent.

## Moved 2026-08-31 -- topology provenance: a topology now carries where it came from, and cannot pass as measured

# Checklist: topology provenance

**Problem.** [crates/windows-topology-sys/src/topology.rs](crates/windows-topology-sys/src/topology.rs)
documents that a `Topology` is "built either by `Topology::discover` from the running system, by hand,
or (with the `serde` feature) by deserializing a fed-in description" -- and **nothing distinguishes the
three once built**. `Topology` derives `Default`, has public fields, and derives `Deserialize`. There is
a passing test that parses a *Linux-shaped* description, complete with an ACPI SLIT-style distance
matrix, on a Windows-only crate. A consumer handed that value treats another machine's topology, or a
fabricated one, as this machine's truth.

This is not hypothetical for the work in flight. `probe-core-affinity` needs synthetic multi-node
topologies precisely because no NUMA machine is available, and the whole point of a probe is that its
output is believed.

**Decision.** Topology content carries its own provenance, defaulting to the *untrusted* value so that
forgetting is safe and claiming is deliberate. Persisted forms carry it visibly, and loading can only
ever downgrade -- a file cannot assert that it is this machine.

Related: [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M-inf.4, which is what surfaced this.

## M1: the marker, and its invariants

- [x] **TP-1.1** -- Add `Provenance` to `windows-topology-sys` with three states ordered by trust:
  `Measured` (read from the running system), `Restored` (deserialized from a description of some
  machine), `Synthetic` (constructed by hand). **`Synthetic` is `Default`.** That is the load-bearing
  choice: `Topology::default()`, `..Default::default()`, and any construction that omits the field all
  come out tainted, so a caller must do work to claim data is real rather than work to admit it is not.
  Document that the threat model is *accident*, not forgery -- a caller who writes
  `provenance: Measured` over fabricated data has lied deliberately, and no type prevents that.

- [x] **TP-1.2** -- Add the field to `Topology` and set `Measured` in `discover()`. This is a **breaking
  change** for struct-literal construction, and deliberately so: every existing site is forced to state
  which kind of data it holds. Update the crate's own tests and every dependent that constructs a
  `Topology` by hand.

- [x] **TP-1.3** -- Serde: serialize the marker so it is *visible* in the persisted form, and
  **downgrade on load** -- `Measured` becomes `Restored`, everything else is unchanged. The rule is
  **never upgrade**, so a hand-edited `"provenance": "measured"` is ignored rather than honoured. A
  description absent the field loads as `Synthetic`. Test each of the four load cases, including that a
  round trip of a measured topology does not come back measured.

## M2: making it loud where it is read

- [x] **TP-2.1** -- `Fingerprint` in [crates/windows-placement-probe/src/fingerprint.rs](crates/windows-placement-probe/src/fingerprint.rs)
  carries the provenance and renders it **first and unmissably** when it is not `Measured`. The
  fingerprint string is documented as canonical, so string equality is a usable comparison -- which
  means the marker must be *inside* the string, or a synthetic host could compare equal to a real one.
  That is the specific bug this prevents, not merely a display nicety.

- [x] **TP-2.2** -- Every probe banner and every persisted probe line inherits it, since
  `print_banner` and `Slice` are what end up pasted into checklists and design notes. A number quoted
  from a synthetic run must arrive already labelled, because the label is what a reader will not think
  to ask for.
  **Done, and the banner inherits it by construction** -- it embeds the fingerprint's own `Display`
  rather than re-rendering, so the two cannot drift. `print_banner` was split so the line is available
  as a string (`banner_line`) and the marker's arrival is asserted rather than confirmed by reading a
  format string.
  **`Slice` deliberately carries no marker of its own, and the reason is structural rather than an
  oversight.** A `Slice` records which processors a measurement was pinned to, and one can only exist
  from a real `measure()` run: `measure` takes no injected topology (and
  [crates/windows-placement-probe/src/core_affinity.rs](crates/windows-placement-probe/src/core_affinity.rs)
  now documents why it must not), and pinning to a processor that does not exist panics. A slice is
  therefore always real, and it is always printed beneath the banner that carries the host's
  provenance. **If `measure` ever does gain such a seam, this reasoning collapses and `Slice` needs its
  own marker** -- which is a second, independent reason not to add one.

## M3: closing the loop with the probes

- [x] **TP-3.1** -- Reconsider whether `probe-core-affinity`'s synthetic hosts should be expressed as
  `Topology` values rather than as `Vec<ProcessorPlace>`. Going through `Topology` would exercise the
  provenance path end to end and let a synthetic *NUMA* host drive selection through the real
  `discover_places` conversion; staying at `ProcessorPlace` keeps the tests pure and fast. **Decide on
  the evidence, and record the decision either way** -- this item is not "do it", it is "choose".
  Note the constraint from
  [crates/windows-placement-probe/src/core_affinity.rs](crates/windows-placement-probe/src/core_affinity.rs):
  `measure()` must still not gain a topology-injection seam, whatever is decided here.

  **Decided: both, because they are tests of different units -- and the evidence that settled it was a
  hole, not a preference.** `classify`, `representative_pairs` and `node_pairs` take `ProcessorPlace`;
  that *is* their input type, so `ProcessorPlace` fixtures test them at their own boundary and stay.
  What was missing is that `discover_places` -- which carries the rules for which cache level
  partitions the machine, which core and class each processor belongs to, and which NUMA node -- took
  no argument, called `Topology::discover()` internally, and appeared in **zero tests**. It was not
  merely untested; it was untestable.

  **That hole was load-bearing and is now proven closed.** The NUMA lookup added earlier could not be
  verified on a single-node host, because a correct map and a completely broken one both yield node 0.
  Replacing the whole lookup with a hardcoded `0` was tried against the suite as it stood before this
  item: **it passed everything.** Against the suite now, three tests fail. The `ProcessorPlace`
  fixtures could never have caught it, because they encode what a test author *assumed* the conversion
  produces -- the exact "depend on specified primitives, never on incidental behavior" trap.

  **A pure `places_from_topology` seam was added; `measure()` still has none.** The distinction is the
  rule worth keeping: *a seam that only moves data is safe; a seam that lets fabricated labels reach
  real hardware is not.* Feeding a synthetic topology to a conversion yields synthetic positions, which
  is what the caller asked for and cannot be mistaken for a measurement. Feeding one to `measure()`
  would produce genuine timings under fabricated node ids, because a synthetic topology's processor
  *numbers* are still valid on the real host and every pin would succeed.

## Moved 2026-08-31 -- the sabotage harness became a tool

### <a id="m341"></a>M34.1 -- Promote the ad-hoc sabotage harness into a reusable tool. *(completed 2026-08-31 20:03:57 -04:00)*
- [x] **M34.1** -- Promote the ad-hoc sabotage harness into a reusable tool. **Done.**
  [tools/run-sabotage.ps1](tools/run-sabotage.ps1) plus
  [tools/README-sabotage.md](tools/README-sabotage.md), driven by a `sabotage.json` kept beside the
  code it patches; the first is
  [crates/windows-waitable-queues/sabotage.json](crates/windows-waitable-queues/sabotage.json), whose
  nine entries reproduce the M30.4/M30.5 sweep exactly through the promoted tool.
  Six of the tool's own guards were verified by making each one fire: a name filter matching nothing,
  a missing file, a dirty target, a pattern matching 14 sites instead of 1, a patch that changes
  nothing, and a deliberately red baseline. A harness whose guards are untested is the thing it exists
  to warn about.
  Two subtleties are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `Sabotage sweeps` rather than
  left in the script: a **survived** sabotage may be a defect in the *sabotage* rather than a hole in
  the tests, which is why the patch is now printed on every unexpected result; and a **too-short
  timeout manufactures a false "caught"**, crediting tests with catching a defect they never ran
  against, so the bound errs generous.
## Moved 2026-09-01 -- Thread ambient mutation gaps

### <a id="m236"></a>M23.6 -- Close mutation gaps with deterministic fault injection and exhaustive assertions. *(completed 2026-09-01 20:25:29 UTC-04:00)*

Close the actionable gaps from the 2026-09-01 mutation run. Add deterministic,
thread-local, test-only fault injection at the error-mode, declared-aspect, and transaction OS-call
boundaries; use it to prove explicit release, best-effort drop, rollback, unsupported-platform, and
composite cleanup behavior; add exhaustive unit assertions for error accessors, formatting, sources,
capture-set formatting, declared emptiness, and restore reports; then rerun mutation testing and
classify any survivors that are behaviorally equivalent.

The final mutation run tested 233 mutants: 142 were caught, 91 were unviable, and none were missed.

## Moved 2026-09-02 -- M1 of the topology/queues release: the public surface settled before publication

Every item complete. The milestone existed because its decisions were free before 0.1.0 and expensive
after: a deleted public type costs a yank-and-migrate once published, and `Reserving`'s associated
type needed its bound before any caller could depend on the unbounded form. Moved from
[CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md).

### M1: settle the public surface before it is public

- [x] **SH-1.1** -- **MIRRORS [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M31.8 -- one piece of
  work seen from two plans. Check both off in the same commit; neither is done alone.**
  **Decide M31.8 (merge-or-delete for `slotwise_mpsc` and `reserving_mpsc`) before the first
  publish, not after.** This is the highest-leverage item in the file and it is release-blocking for a
  mechanical reason: the decision may *delete a public type*. Doing that before 0.1.0 costs nothing;
  doing it after means a breaking release, a yank-and-migrate for anyone who adopted it, and a
  permanent line in the changelog explaining why a shape existed for one version.
  The measurement is already done and agrees across both architectures -- see M31.5 and M31.7 in
  [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) -- so this needs a decision, not more work.

- [x] **SH-1.2** -- **GOVERNS [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M31.6 -- this is not
  that item and does not complete it.** It decides only whether M31.6 blocks SH-4.3. If the answer is
  "it gates", record that on M31.6 and SH-4.3 cannot proceed until M31.6 is done; if "it does not",
  record that too, so a later reader does not mistake a considered choice for an oversight. **Checking
  this off never checks off M31.6.**
  **Decide explicitly whether M31.6 (loom verification) gates 0.1.0**, and record the
  answer either way rather than letting it drift into "not yet".

  **Decided: it does not gate 0.1.0. It gates 1.0, and the gap is disclosed in the crate's own
  documentation rather than left for an adopter to discover.** Recorded as D-31.

  Three findings drove it, and the second was not expected:

  - **Loom would close the demonstrated gap.** The sabotage sweep showed a weakened `Acquire` on the
    producer's load of `head` survives the whole suite. That defect lives in queue code, which is
    exactly what loom models well.
  - **Loom would *not* close the gap where a real bug actually occurred.** The doorbell's correctness
    is the interleaving of an `AtomicBool` mirror with real `SetEvent`/`ResetEvent` syscalls. Loom
    models the atomics and cannot model the syscalls; stubbing them tests a *model* of `SetEvent`
    rather than `SetEvent`. D-15's lost wakeup -- the only ordering bug this crate has actually had --
    was found by sabotage, and loom would not have found it. So loom is valuable and is **not** the
    thing standing between this crate and confidence about its hardest part.
  - **The risk loom addresses is mostly regression risk**, and that risk is lowest now. The orderings
    are believed correct and were reasoned about at the time; sabotage *introduced* the weakening to
    prove the suite was blind to it. Regression risk rises with contributors, changes, and consumers
    -- all of which start after publication, not before.

  Against that, gating would block 0.1.0, and through it the placement tool and the NUMA measurements
  from other people's machines that this whole sequence exists to obtain. Loom is invasive work: every
  atomic in the crate goes behind a `cfg` shim across four modules.

  **The disclosure is what makes this a decision rather than a punt**, and it is not optional: the
  crate documentation states what is verified, states that stress testing here is *known* not to catch
  ordering defects and cites the measurement showing it, and says loom is planned before 1.0. An
  adopter then makes their own call with the same information we have. `0.x` carries the rest.
  The reason it deserves a deliberate answer rather than a default: the sabotage sweep demonstrated
  that weakening the producer's `Acquire` load of `head` to `Relaxed` left **all twenty tests green**,
  while every logic defect injected beside it was caught. So this is not an untested-by-omission gap,
  it is a gap this workspace has *evidence* the existing tests cannot close. Publishing a lock-free
  queue with it open is a defensible choice; making it unknowingly is not.

- [x] **SH-1.3** -- **Qualify both MPSC shapes by name.** `mpsc` beside `reserving_mpsc` made one
  canonical by implication -- which contradicts this crate's own "no shape is the canonical one", and
  after SH-1.1 is simply false. Renamed to `slotwise_mpsc`, which names its claim protocol: it claims
  slot by slot with no shared counter. `sequence_mpsc` was considered and rejected for inviting the
  reading that it alone preserves FIFO order, which both shapes do. Recorded as D-30.
  **Belongs in M1 for the same reason SH-1.1 does**: it is a public-surface change, free before the
  first publish and a breaking rename with a deprecation path afterwards.

- [x] **SH-1.4** -- **State the algorithms' pedigree and why an existing crate is not used.** A public
  concurrent-queue crate has to answer both questions or a reader assumes the worst: that the
  algorithms are homegrown, and that the author did not look at the alternatives.
  Neither is true, and the honest answers are load-bearing. The algorithms are *published designs*
  chosen deliberately, because a concurrent queue is a bad place to be original -- the failure mode is
  a reordering that appears on one machine, under load, months later. And the reason no channel crate
  fits is structural rather than dismissive: **on Windows, waiting is a kernel-object operation**, so a
  queue whose readiness is not a `HANDLE` cannot join a `WaitForMultipleObjects` alongside an I/O
  completion, a process handle, or a cancellation event -- however good its own blocking receive, and
  however rich its own `select`, which can only select over its own channels.
  Written into both the crate docs and the README, because docs.rs shows one and crates.io the other.

- [x] **SH-1.5** -- **Bound `Reserving::Reservation<'a>` so a generic caller can redeem what it claims.**
  Done: the `Claim` trait carries `send` and `is_disconnected`, `Reservation<'a>` is bound on it, and
  both reservation types implement it as forwarders. 87 lines across five files, no concrete signature
  changed, `slotwise_mpsc` untouched because it does not implement `Reserving` at all. Mutation-tested
  rather than assumed: 62 mutants over the whole reservation surface report 0 missed, and the first
  run found a real gap -- `is_disconnected` stuck at `false` survived on `spsc`, because the connected
  case was asserted there and the disconnected case only on the other shape.
  The associated type is declared with no bound at all, so a caller generic over
  [`Reserving`](crates/windows-waitable-queues/src/traits.rs) can call `reserve()` and then do
  nothing with the result except drop it. `reserve` is `#[must_use]` precisely because a held claim
  withholds capacity from every other producer -- and the one operation that discharges it, `send`,
  is inherent to each shape's concrete type and unreachable through the trait. The trait cannot
  express the operation it exists for.

  **The two implementors already agree exactly, so this is additive**: both
  `spsc::Reservation<'a, T>` and `reserving_mpsc::Reservation<T>` already have
  `send(self, item: T) -> Result<(), Disconnected<T>>`, `is_disconnected(&self) -> bool`, and a
  `Drop` that returns the slot. No concrete signature changes; nothing to migrate.

  Add a `Reservation` trait carrying `send` and `is_disconnected`, bound the associated type on it,
  and implement it for both types. `is_disconnected` is included rather than deferred for the reason
  the `Reserving` docs give at length -- a caller needs to learn the stream ended *before* doing the
  work the claim was taken for -- and because `reserving_mpsc`'s reservation is `Send`, so it may be
  redeemed on a thread holding no producer handle to ask instead. Adding it later is the same
  breaking change, merely deferred.

  **Why this blocks rather than waits.** Adding a bound to an associated type is a breaking change
  to the trait: every implementor must then satisfy it. It is free while the crate is unpublished
  and a major bump with a migration afterwards, and this is the milestone that exists to settle
  exactly that -- see SH-1.1 and SH-1.3, both landed on the same "free before the first publish"
  reasoning. D-3 already makes this argument ("the trait *shape* is fixed now so signatures stay
  compatible"); this is the same reasoning applied to a piece it missed. Pull request #56 is what
  puts these traits in front of consumers, so the window closes when it merges.

  **How it surfaced**, recorded because the route is the useful part: not from review and not from a
  failing test, but from a `cargo mutants` run showing that nothing exercised the capability traits
  at all, and then from being unable to write the obvious generic test for `Reserving` -- the test
  in [traits/tests.rs](crates/windows-waitable-queues/src/traits/tests.rs) is scoped to claim-and-release
  and says so. A contract gap presenting as an untestable API is a signal worth keeping.

  Extend that test to claim-and-redeem through the trait as part of this item, since it is the
  check that would have caught the gap in the first place.

## Moved 2026-09-02 -- M7 through M13: seven PR #56 review rounds, all findings resolved

Seven consecutive automated-review rounds on the pull request that ships `windows-topology-sys`
0.2.0 and `windows-waitable-queues` 0.1.0, kept as milestones so each round's findings stayed
attributable to the round that raised them. All items complete. Moved from
[CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md); the two later rounds
(M14, M15) remain there because they carry open work.

### M7: PR #56 automated-review round

The findings an automated review raised against the pull request that lands this work, verified against
the source before being accepted. Each item names what was checked, so a later reader can tell a real
repair from a reviewer's guess that was taken on trust.

- [x] **SH-7.1** -- **`reserving_mpsc` reports `Full` from a claim word that was never current.**
  `push` and `reserve` load the claim word relaxed, then test room with
  `has_room_beyond_reservations(position, reserved)`, which computes
  `position.wrapping_sub(head)`. If other producers claim and publish past `position` and the consumer
  drains them while this thread is between the load and the room check, `head` passes the stale
  `position` and the subtraction wraps to near `u32::MAX` -- so the queue reports `Full` (and records a
  refusal) at the moment it is empty, and `reserve` returns `None` for the same reason. The compare-and-
  swap that would have caught the staleness is never reached, because both paths return before it.
  Re-read the claim and retry when it moved; report no room only from a word still current.

- [x] **SH-7.2** -- **The NUMA cross-check compares a count against a highest identifier.**
  `windows-platform-probes`'s `Observation::cross_check` compares `numa_domains` (a count of memory
  domains) with `GetNumaHighestNodeNumber() + 1`. Windows documents that value as the highest node
  *number*, and does not guarantee node numbers are dense -- nodes 0 and 2 give a count of 2 and a
  highest of 2, and the probe then reports a parsing regression on correct hardware. Memory domains
  already carry the node number in `Domain::id`, so compare highest against highest.

- [x] **SH-7.3** -- **A cache level is called a partition without checking that it is one.**
  `cache_partitions_at_level` deduplicates by equal processor set, which is exactly right for the
  measured case it was written for (L1i and L1d over identical sets). It does not establish a
  *partition*: `Topology` is deliberately constructible by hand and by deserialization (D-12), so
  distinct-but-overlapping sets reach `outermost_partitioning_cache`, which returns them as domains a
  consumer then double-counts. Require the distinct sets to be pairwise disjoint before a level
  qualifies as partitioning.

- [x] **SH-7.4** -- **`windows-waitable-queues` cannot build its documentation on docs.rs.** The crate
  is Windows-only and imports `std::os::windows::io` unconditionally, but its manifest omits the
  `[package.metadata.docs.rs]` target block that every other published Windows-only crate here carries,
  so docs.rs would build it for its default Linux target and fail. Add the same block.

- [x] **SH-7.5** -- **The mutant injector replaces every occurrence on the line, not the first.**
  `tools/inject-mutant.ps1` calls the *static* `[regex]::Replace(input, pattern, replacement, 1)`, whose
  fourth parameter is `RegexOptions` -- `1` is `IgnoreCase`, not a replacement count, and no static
  overload takes a count at all. The tool therefore does precisely what its own header comment says it
  exists to avoid. Fix the replacement, refuse a line whose pattern occurs more than once unless a
  column disambiguates it, verify the baseline is green before trusting a "caught", run with all
  features so a feature-gated mutation is not reported as surviving, perform the mutating write inside
  the guarded region so a failed write still restores, and route its output through one sink.

- [x] **SH-7.6** -- **A spike that fails to run is reported as a finding about the machine.**
  `tools/run-numa-spikes.ps1` checks the exit code of `cargo build` but not of `cargo run`, then decides
  vacuity by searching the output for `VACUOUS`. A crashed spike prints no such line, so the summary
  says "**NOT vacuous -- this runner has more than one NUMA node**" and the script exits 0. That is the
  instrument breaking while claiming a result, which the script's own documentation says is the one
  thing worth failing over.

- [x] **SH-7.7** -- **Two tools write output from several sites, and two hazards remain in the
  sabotage/mutation harness.** `tools/check-publishable.ps1` and `tools/inject-mutant.ps1` each call
  `Write-Host` from several places, against the repository's one-output-sink rule.
  `tools/run-sabotage.ps1` performs its patching write before entering the `try` whose `finally`
  restores the file, so a write that throws part-way leaves the clean source damaged.
  `tools/run-mutants.ps1` derives a deterministic output directory per package or file, so a second run
  of the same scope overwrites the analysis the parameter documentation promises to preserve.
  The placement probe's tests name scratch directories without the process id, so two concurrent test
  processes -- which the documented `-j 2` mutation workflow creates -- delete each other's fixtures.

- [x] **SH-7.8** -- **Reply to every thread and resolve the ones that are addressed**, including the one
  finding that was checked and found not to hold: `GetSystemDirectoryW` returning exactly the buffer
  length is unreachable (success excludes the terminator, failure includes it and so exceeds the
  buffer), though the guard is widened anyway so the next reader need not redo the analysis.

### M8: PR #56 third review round (suppressed findings)

The reviewer generated no new inline comments in these rounds and instead listed **suppressed** findings in
the review body, so none of them arrived as a resolvable thread. They are recorded here because a finding
that produces no thread is otherwise invisible to the "are all comments resolved?" check that gates merge.

- [x] **SH-8.1** -- **The contention probe times thread creation, and lets early producers run alone.**
  All five timed runs in `windows-platform-probes`'s `queue_contention` start the clock *before*
  `thread::scope` spawns anything, and every worker begins pushing the moment it is spawned. At 50,000
  pushes each, an early producer can finish a large uncontended prefix -- or finish outright -- while the
  last threads are still being created, so a row labelled 16 or 32 producers may never have had 16 or 32
  contenders. The measured interval also includes spawn cost. This is not a cosmetic inaccuracy: the
  module's own header says these numbers decide whether two speculative queue shapes get written at all
  and whether the two shipped shapes merge. Hold every participant -- producers *and*, in the drained
  runs, the consumer -- at a start barrier, and start the clock when it releases.

- [x] **SH-8.2** -- **A failed backup write leaves a truncated file under the canonical name.**
  `write_backup_to_new_file` reserves the name with `create_new` and then `write_all`s through `?`, so a
  disk-full or quota failure returns an error while leaving a zero-length or partial `.json` behind. That
  file is indistinguishable from a real record to whoever collects it, and the next run's collision
  suffix steps politely around it. Publish by rename: write the bytes to an exclusively-created temporary
  in the same directory, flush, and move it onto the reserved name only once the write has succeeded.

- [x] **SH-8.3** -- **`places_from_topology` drops processors and invents NUMA membership.**
  Two defects in one conversion, both reachable only through a hand-built or deserialized `Topology` --
  which is exactly the input this seam exists to accept (D-12).
  It iterates `class_of`, which is populated only from `DomainKind::Core` domains, so an online processor
  with no core domain is **silently absent from the result** -- and the documented core-id fallback
  beneath it, written to keep group 1's cpu5 distinct from group 0's, is unreachable dead code as a
  direct consequence.
  It then defaults absent NUMA membership to `unwrap_or(0)`. That is the right answer only when the
  topology names no memory domain at all; when it names nodes 1 and 2, it **fabricates node 0** and files
  a processor under a node the machine does not have -- the precise failure this crate's own rule
  ("a seam that only moves data is safe; a seam that lets fabricated labels reach real hardware is not")
  exists to prevent.
  Iterate the online processors so every one is placed, and refuse a topology that names memory domains
  but not this processor's, rather than inventing one.

### M9: PR #56 fourth review round

- [x] **SH-9.1** -- **Both bounded shapes could report a length larger than their capacity.**
  `len` reads the producer-side position and then `head`, which are two instants; a consumer draining
  past the sampled position makes the wrapping subtraction yield a number near the integer maximum. The
  comment beside it claimed the overestimate was "safe in the direction that matters for a backpressure
  gauge", which is true of a *bounded* overestimate and not of `usize::MAX`. Both are now clamped to the
  capacity, so the skew still resolves towards full -- the safe direction -- while the impossible value
  is gone.

- [x] **SH-9.2** -- **`reserving_mpsc` inherited a `remaining()` that counted reserved slots as room.**
  `Bounded::remaining` defaults to `capacity - len`, and this shape's `len` excludes reservations by
  design, so an empty queue of four holding one reservation answered four while only three items fit --
  promising room for a push guaranteed to be refused. Overridden on both handles and both trait impls,
  reading the packed claim word **once** so the position and the reservation count cannot be sampled at
  different instants; `is_full` is now defined in terms of it rather than restating the rule.

- [x] **SH-9.3** -- **The pull request description described the release plumbing, not the product.**
  The body framed the change as CI and provenance work and mentioned `windows-waitable-queues` only
  under release tracking, while the majority of the diff is that crate's public API and its three
  lock-free queue implementations. Rewritten to lead with the shipped surface.

### M10: PR #56 fifth review round

- [x] **SH-10.1** -- **`BOUNDS_MAX` does not compile on a 32-bit target.** `reserving_mpsc`'s maximum
  was a flat `1 << 31`, derived from the packed position's width alone. On a 32-bit target the
  crate-wide `WRAPPING_MAX_CAPACITY` is `usize::MAX / 2`, which is `2^31 - 1` -- *narrower* than the
  packing -- so the const assertion that no shape may exceed it fails the build outright, for every
  capacity including the small valid ones. Now the narrower of the two limits, kept a power of two so
  the value stays one a caller could actually pass. Verified in both directions against a real
  `i686-pc-windows-msvc` check: the old constant fails with `E0080`, the new one compiles.

- [x] **SH-10.2** -- **The backup's final name was visible empty for the whole write.** The previous
  round reserved the destination with `create_new` and renamed onto it, which fixed the truncated-file
  case and left a worse one: an empty file under the record's own name for the duration of the write,
  and permanently if the process was killed in that window -- contradicting the absent-or-complete
  guarantee its own doc comment claimed. Publication is now a single atomic no-replace `MoveFileExW`
  from a fully-written temporary. `std::fs::rename` cannot express this: on Windows it always passes
  `MOVEFILE_REPLACE_EXISTING`, so it would clobber a record a concurrent run had placed.

- [x] **SH-10.3** -- **The tool discovered the topology three times.** The plan used one reading, the
  fingerprint another, and `core_affinity::measure` a third, so a processor going offline mid-run could
  have the announced plan, the recorded host, and the measured rows describing different machines with
  nothing saying which. The plan and the fingerprint now derive from one `Topology::discover`.
  `measure` still discovers its own, and deliberately so: its documentation refuses a
  `measure_with(places)` seam because a supplied list's processor *numbers* stay valid on the real host
  while its node labels need not, so every pin would succeed and real timings would be filed under
  fabricated labels. Its rows carry their own places, so each row states what it measured.

- [x] **SH-10.4** -- **`spsc` had the same `remaining()` defect, and it was missed.** The previous round
  corrected `reserving_mpsc` and stopped there, but `spsc` implements `Reserving` too -- so reserving
  every slot left it reporting the full capacity as available while both `push` and `reserve` refused.
  Its `Bounded` impls now override `remaining` on the producer *and* the consumer, its `len` is clamped
  to the capacity like the other two shapes', and `is_full` is defined in terms of `remaining` rather
  than restating the rule. The trait's default now documents that a `Reserving` shape must override it,
  so the next shape to reserve does not inherit the same wrong answer silently.

- [x] **SH-10.5** -- **The high-water depth could record a peak the queue never reached.**
  `reserving_mpsc`'s `publish` sampled the depth from its own position and a relaxed load of `head`,
  ungated and unclamped. `slotwise_mpsc`'s twin is bounded by construction -- its producer's acquire
  load of the slot's sequence synchronizes-with the consumer freeing that slot, so `head` cannot be
  older than `position - capacity + 1` -- but this shape has a second entry point with no such edge:
  `Reservation::send` redeems without a room check, so the only `head` its thread is ordered against is
  the one *`reserve`* read, which may be arbitrarily old by the time the reservation is redeemed. The
  sample is now gated on tracking (parity with the twin), read before publication, and clamped to the
  capacity.
  `Observable::high_water`'s contract is corrected to match what all three shapes actually deliver: an
  **upper bound** on the true peak, never below it and never above the capacity, with the reason the
  cheap sample is preferred to an exact count. Counting exactly would put a read-modify-write on a line
  shared by every producer and the consumer into every push and every pop -- the line this crate pads
  its positions apart to keep out of the hot path.

### M11: PR #56 sixth review round

Three findings against `places_from_topology`, all of the same shape, plus three against the
mutation wrapper. The conversion's three silent fallbacks are replaced by one rule.

- [x] **SH-11.1** -- **Three fallbacks each invented an answer that reads as a real one.**
  `places_from_topology` accepted a topology whose domains do not cover every processor, and filled
  each gap with a value indistinguishable from a measured one. A processor absent from every core
  domain was given a synthetic core id derived from its group and number, which can equal a real
  core domain's id -- `classify` then reports two processors as SMT siblings when one's core is
  merely unknown. Its efficiency class became `0`, which is also a genuine Windows class, so
  `within_class_pair` reports a same-class pair against a real class-0 core. Its cache domain became
  `None`, which the type already means "no cache level partitions this machine" -- so two processors
  omitted from an incomplete partition compare equal and serialize a confident same-cache
  measurement.
  The three share one cause: an absence was read as a value. The rule now distinguishes *uniform*
  absence from a *gap*. A machine that reports no core domains at all, or no partitioning cache
  level, has told us something true about itself and still converts. A machine that places every
  other processor but not this one has told us nothing about this one, and the conversion refuses:
  `places_from_topology` returns `Err(UnplacedProcessor)` naming the processor and, in a new
  `MissingPlacement` field, which of core / cache domain / NUMA node was missing.
  `MissingPlacement` is `#[non_exhaustive]`.
  Core and efficiency class are two spellings of one rule -- `Topology::cores()` filters to
  `DomainKind::Core`, so a processor's class is known exactly when its core is -- and an
  `EfficiencyClass` variant written for the second was removed on discovering it is unreachable.
  Sabotage confirms the pair behaves that way: removing either refusal alone leaves the suite green,
  because the other still fires; removing both fails two tests.

- [x] **SH-11.2** -- **The mutation wrapper's output directory could collide.** The stamp has
  one-second resolution, so two runs launched in the same second -- a script starting several scopes
  at once, which is exactly the case that wants separate output -- selected the same directory and
  interleaved their results. A short random suffix now follows the stamp, which still sorts
  chronologically.

- [x] **SH-11.3** -- **The wrapper terminated fault handlers it did not start.** Cleanup matched
  `WerFault` / `WerFaultSecure` / `vsjitdebugger` by name across the whole session, so a crash report
  the user was reading or a debugger attached to an unrelated process was killed by a mutation sweep.
  The wrapper now records the ones already running at startup and skips them.

- [x] **SH-11.4** -- **The `mutants.out` nesting finding does not hold; documented in place.**
  The report was that `Join-Path $OutputDirectory 'mutants.out'` doubles a path cargo-mutants already
  appends. It does not: cargo-mutants treats `--output` as the parent and creates `mutants.out`
  inside it. Verified on disk -- a run with `--output .scratch\mutants-encoding-<stamp>` produced
  `<stamp>\mutants.out\caught.txt` with 22 lines, matching the 22 caught the wrapper reported. A
  comment now records the evidence, since the path reads like a duplication and has been challenged
  once already.

- [x] **SH-11.5** -- **A hard-killed run could block the next one's backup entirely.** The temporary
  was named for the record plus this process's id and nothing else, and created with `create_new`. A
  run killed mid-write leaves that file behind, and Windows reuses process ids -- so a later run
  issued the same id found the corpse under the only name it would ever try. The resulting
  `AlreadyExists` left `write_temporary` *before* the caller's suffix loop was reached, so the whole
  backup failed rather than landing under a next-best name. The temporary now carries its own
  attempt counter, matching the final name's budget; a stale file is stepped around rather than
  overwritten, since it belongs to whatever left it.

- [x] **SH-11.6** -- **Both tools bypassed their own single-output-sink contract.** `run-mutants.ps1`
  emitted its per-category summary directly to the success stream, and `run-sabotage.ps1` did the
  same for the `-List` output, every blank line, the result table, and the injected patch text --
  each contradicting the `Write-Report` doc comment directly above them. All now route through the
  sink, which gained pipeline binding so a formatted table can flow into it. Verified: the `-List`
  path emits zero objects to the success stream.

- [x] **SH-11.7** -- **The sabotage harness silently narrowed its own sweep.** Found while checking
  SH-11.6's output: the harness prepends the `test` subcommand to a manifest's `testArgs`, but nine
  of the eleven manifests already begin with `test`. The result was `cargo test test -p ...`, in
  which the second word is not a subcommand but a TESTNAME filter -- a sweep claiming to run a
  package's suite while running a subset of it, the same false-green this tool exists to prevent.
  The vector is now normalised so either manifest spelling produces one `test`.
  **No prior verification was weakened**: every crate swept so far keeps its tests under a
  `mod tests`, so the accidental filter matched all of them, and all fourteen recorded baselines
  report "0 filtered out". The defect was latent, and would have appeared on the first crate laid
  out differently.

### M12: PR #56 seventh review round

- [x] **SH-12.1** -- **The banner undercounted the machine it was about to measure.**
  `Fingerprint::processors` is documented as the logical-processor count but was summed over
  core-domain membership, which agreed with that meaning only while every processor was guaranteed to
  sit in a core domain. SH-11.1 stopped guaranteeing it: `places_from_topology` now explicitly accepts
  a topology naming no cores and places every online processor. The banner consequently read
  `0p/0c` for a machine the measurement was about to use four processors on -- a defect this branch
  created rather than inherited.
  The count is now read off `topology.processors` with the same `online` filter the placement applies,
  so the summary counts exactly what the measurement will use. `cores` deliberately still counts core
  domains: zero there is the honest report that the topology named none.
  Sabotage-verified. Two new tests pin both directions -- an uncored processor is still counted, an
  offline slot is not -- and each asserts equality against `places_from_topology`'s own output rather
  than a literal, so the two cannot drift apart again.

- [x] **SH-12.2** -- **Two consequences of that fix, found by sweeping it rather than reported.**
  `cache_domain_sizes` fills itself with the processor count when no cache level partitions the host,
  so the bare-machine render silently improved from `L-[0]` to `L-[4]`.
  `numa_node_sizes` did not, and now does not sum to `processors` in that one case: it reports the
  nodes the topology *named*, and a bare topology names none, while every placement still reports the
  documented node-`0` default. Keeping it that way is deliberate -- the cache list can afford to fill
  itself because `L-` marks the absence, and `numa[4]` would be indistinguishable from a host that
  genuinely reported one node of four. The behaviour is now documented on the field and pinned by a
  test that asserts the whole render, including the `!!SYNTHETIC!!` provenance marker. The stronger
  fix needs a marker, which is a serialized field and so a schema bump, tracked as
  [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) `PT-6.2`.

### M13: PR #56 eighth review round

- [x] **SH-13.1** -- **A record could splice two machines together.** The tool announced a shape read
  at one instant while `core_affinity::measure` discovered again at another, so a processor going
  offline -- or moving group or node -- between them produced a record whose `host` described one
  machine while every row was measured on a different one. Nothing in the file said so, and the host
  is precisely what a reader interprets row sets *through*.
  `measure` now reports the shape it actually ran on, as `Observation::host`, which is the fix that
  keeps the anti-synthetic boundary intact: the measurement still discovers for itself and no seam
  accepts a fabricated shape from outside. `SubmissionRecord::new` refuses when the announced and
  measured hosts differ, so the splice is unrepresentable rather than merely avoided at the one
  current call site; the tool checks first anyway and reports the disagreement in terms a runner can
  act on. Refusing rather than silently recording the measured shape, because the notice is what the
  runner consented to.

- [x] **SH-13.2** -- **The tool wrote from 54 independent print sites.** The repository's
  one-output-sink rule requires an output abstraction at the *first* output site so the storage
  target and the formatting stay separable from the call sites that compose content. This binary had
  none, which is why its collection notice -- a disclosure a runner reads before agreeing to publish
  facts about their machine -- could only be exercised by running the process and capturing stdout.
  A `Sink` trait now carries the two streams the tool genuinely has, `print_collection_notice` and
  `print_plan` became `render_*` functions returning a `String` (matching the idiom the record report
  already used), and `main` is the only place that names the real streams.
  Verified as a pure refactor by comparing the built binary's output before and after: `--preview`
  and `--help` are **byte-identical**, and `--version` differs only by the build identity correctly
  reporting the working tree as `DIRTY`. Eight new tests cover what was previously unreachable,
  including that the notice shows the model rather than describing it, that a withheld model reads
  differently from one the host would not report, and that the two streams cannot satisfy each
  other's assertions.

- [x] **SH-13.3** -- **The two new probes wrote from 94 independent print sites between them.** Same
  rule as SH-13.2, in [core_affinity.rs](crates/windows-platform-probes/src/bin/core_affinity.rs)
  (67 sites) and [doorbell_cost.rs](crates/windows-platform-probes/src/bin/doorbell_cost.rs) (27).
  A `Report` sink now lives in [report.rs](crates/windows-platform-probes/src/report.rs), shared by
  both. One stream, not two: unlike the placement probe these have only ever written to stdout, and
  inventing a diagnostic stream they do not use would be adding a distinction the tools do not make.
  Each `main` is now three lines -- measure, render, emit -- and is the only place naming the real
  stream.
  One find during the conversion that the mechanical part would have missed: `render` called
  `fingerprint::print_banner()`, which writes to stdout *itself*. Left alone it would have put the
  identifying line on the terminal while leaving it out of the returned report, so a captured report
  would be missing the one line saying which machine produced it -- and the `!!SYNTHETIC!!` taint
  marker with it. `banner_line()` already existed for exactly this and is now used.
  Verified as a pure refactor by running both probes before and after and comparing with numerals
  masked (their output is timing-dependent, so byte equality is not available): 38 lines and 50 lines
  respectively, **structurally identical** both times.

- [x] **SH-13.4** -- **The other twelve probes still print directly, and now there is a sink to
  adopt.** `probe-peer-index-cache` (55 sites), `probe-request-cost` (45), `probe-topology` (32),
  `probe-queue-contention` (27), `probe-ioring` (24), `probe-completion-port` (22),
  `probe-worker-context` (22), `probe-device-map` (21), `probe-cancel-io` (19),
  `probe-pool-growth` (16), `probe-handle-state` (14), `probe-error-mode` (10) -- 307 sites.
  Deliberately **not** done in the review round that introduced the sink: those probes predate it and
  are outside that round's scope, and each conversion needs its own before/after comparison against
  the probe's real output, which is what makes it a refactor rather than a rewrite.
  Queued rather than left as a note precisely because a half-adopted abstraction is the state most
  likely to be forgotten -- the next probe author will see twelve neighbours printing directly and
  reasonably conclude that is the house style.