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
## Moved 2026-09-07 -- M34.4: the native-command guard became shared support, proven on both hosts

### <a id="m344"></a>M34.4 -- Share the native-command guard through a dot-sourced `tools/common.ps1`, route every capture site through it, and prove it on both PowerShell hosts. *(completed 2026-09-07 21:16:31 -04:00)*

Queued as a decision rather than a fix: the remaining sites needed a third and fourth copy of
one guard, and `tools/` had no sharing convention. The decision was to adopt one, and finding
out *which* one is the substance of this item.

**Under Windows PowerShell 5.1, a native command that writes to stderr while
`$ErrorActionPreference` is `Stop` raises a terminating error when its stderr is redirected
with `2>&1`.** PowerShell 7 does not, which is why this class survives review and CI.

**The obvious answer -- a `.psm1` -- is wrong, and wrong invisibly.** A scriptblock carries
the session state it was created in, so `Invoke-Native { cargo build }` runs in the caller's
scope while a module copy flips the preference in the module's scope; the flip never reaches
the call. Measured with the identical body in a module: **under 5.1 seven of eight cases
failed, while all eight passed under PowerShell 7.** Dot-sourcing puts the function in the
caller's own scope, where the plain assignment does reach the call. A module *can* be made to
work via `$PSCmdlet.SessionState.PSVariable.Set(...)` -- measured working on both hosts -- and
was rejected: the guard would rest on a subtlety that looks removable, and simplifying it back
reintroduces a defect that still passes on PowerShell 7.

Delivered:

- **[tools/common.ps1](tools/common.ps1)** -- `Invoke-Native` and `ConvertTo-OutputLines`, with
  the "why not a module" argument and its measurement at the definition site.
- **[tools/test-common.ps1](tools/test-common.ps1)** -- eight cases covering capture, stream
  merging, exit-code survival, diagnostic text, record flattening, preference restoration
  (including when the command throws), and that `Stop` stays armed for non-native errors. It
  runs its cases in the invoking host, then **re-invokes itself in the other one**, and treats
  a missing host as a FAILURE rather than a skip -- a single-host pass is not the claim the
  file exists to make.
- **Every capture site routed**: `run-numa-spikes.ps1` and `soak-flush-barrier.ps1` lost their
  local copies; `run-sabotage.ps1` (`check-ignore`) and `test-run-sabotage.ps1` (`init`,
  four `add -A`, and its child-process harness invocation) now go through the shared guard.
  `run-mutants.ps1` needs none -- it redirects nothing, confirmed by experiment on both hosts.
- **CI runs both shells.** The `sabotage harness tests` job ran `shell: pwsh` only, which is
  precisely why the defect was invisible; it now runs `test-common.ps1` plus
  `test-run-sabotage.ps1` under **both** `pwsh` and `powershell`.

Verified by sabotage: delivering the identical guard as a module turns the new suite red on
5.1 (7 of 8) while staying green on 7, so the suite detects the regression it was written for.
Both consumer scripts and the full sabotage suite pass on both hosts.

**Deliberately not done: consolidating `Write-Report`.** Six scripts define one, and they are
not duplicates -- they differ in level vocabulary (`warn`/`warning`, `bad`/`error`, plus
`good`, `note`, `detail`, `heading`) and in rendering, with two emitting GitHub Actions
annotations and four emitting console colours. Merging them would change six tools' output to
remove a duplication that is only apparent. Recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md#tools-shared-support) so it is a decision rather than an
oversight.

## Moved 2026-09-07 -- M35.1: what the long-path opt-in actually does

### <a id="m351"></a>M35.1 -- Measure whether the long-path opt-in lifts `MAX_PATH` for a relative path, and whether it does so without re-parsing it. It does both, and the regularize-then-prefix hypothesis is falsified. *(completed 2026-09-04, archived 2026-09-07 21:40:23 -04:00)*

**Measure whether the long-path opt-in lifts `MAX_PATH` for a *relative* path, and whether it does
so without changing how the path is parsed.**

**Done 2026-09-04, and it settles a question that had produced three wrong answers from reading.**
`probe-long-path-aware` and `probe-long-path-unaware` in
[windows-platform-probes](crates/windows-platform-probes/src/long_path.rs) are the same code
differing only in whether their manifest declares `longPathAware`; `build.rs` embeds it into that
one binary via `rustc-link-arg-bin`, so every other probe binary is unaffected.

**Result, on a host with `LongPathsEnabled=1`.** With the opt-in, a relative path of 429 characters
opens in every shape -- plain, containing `b\..`, and forward-slash separated. Without it, all three
are refused with `ERROR_PATH_NOT_FOUND` while the same shapes at 78 characters open. The targets are
created first, so a not-found from a file that provably exists is the length refusal.

**So the documented reading was right and the review finding was wrong**: the opt-in covers relative
paths, and `MAX_PATH` binds them only in a process that has not opted in.

**And the regularize-then-prefix hypothesis is falsified.** If the opt-in worked by prepending
`\\?\`, that prefix would disable `.`, `..` and forward-slash translation, so those shapes would
have failed past the ceiling while working below it. Both resolve at both lengths. The opt-in lifts
the length check without re-parsing, so there is no discontinuity at `MAX_PATH` for a caller of
`windows-file-watcher` to fall into.

The measurement is recorded where the claim lives, in `Session::subscribe`'s note.

*(Archiving note: the original body said the embed leaves "the other thirteen probes" unaffected.
There were sixteen probe binaries by the time this was archived, so the count was already stale and
would have been written into the record as a false number. Replaced with the count-free phrasing,
which stays true as probes are added. Raised in review 5125955392 on pull request #56.)*

## Moved 2026-09-09 21:28:45 -04:00 -- M22-M29: two new crates, the probes crate, and the defects the audit of them found

From [CHECKLIST-thread-ambient.md](CHECKLIST-thread-ambient.md), whose remaining M26+ items are parked
rather than pending: each is gated on the namespace-facility design branch reaching `main`.

## M22 -- `windows-thread-ambient-sys`: decisions and per-aspect primitives

The captured-context composite is extracted into its own crate and lands **before** M19-M21, despite the
higher milestone number -- the numbering records authoring order, not execution order. The trigger is the
one the imported decision named: an independent consumer exists that needs to carry a caller's ambient
state onto another thread without any of the namespace facility around it. The crate is a *level*
platform, so it offers each aspect for capture **and** for explicit declaration, and does not bake in the
namespace facility's dialog-suppression policy; that policy is composed by the facility from primitives
this crate provides.

Scope boundary, stated so the crate cannot swell: it carries thread-scoped ambient state that changes what
a Win32 call does. It does not carry request parameters, does not open files, and does not know what a
namespace operation is.

- [x] **M22.1** -- Record the extraction decision and the WOW64 correction in
  [DESIGN-NOTES.md](DESIGN-NOTES.md), sweeping every statement of each rather than the one site a reader
  happens to notice. Two changes. First, the composite is extracted **now**, into
  `windows-thread-ambient-sys`: the imported text says it "lives in the facility's crate" and is "not
  extracted preemptively", which was written when the facility was its only consumer, and an independent
  consumer is exactly the trigger that decision named. Second, WOW64 filesystem redirection moves from
  **transplanted** to **declared**, because `Wow64DisableWow64FsRedirection` has no getter -- there is no
  value to transplant, so the transplanted classification was not implementable. That dissolves the WOW64
  half of the session's open question rather than leaving it standing, and the open question must be struck
  in the same commit.
  **Landed together with M22.3, and the coupling is a defect in this plan rather than a convenience:** the
  correction's authoritative statement links to the new crate's `DESIGN-NOTES.md`, so writing it before the
  crate existed would have created a broken cross-reference. Sequencing M22.3 first would have been the
  correct plan.
- [x] **M22.2** -- Measure which `SEM_` bits `SetThreadErrorMode` actually accepts, because it decides
  which bits this crate can offer as declarable. The documented set is three bits and excludes
  `SEM_NOALIGNMENTFAULTEXCEPT`, which is process-scoped and sticky once set. If measurement confirms that,
  M21.2's second sub-question dissolves rather than needing an ARM64/x64 pair, and M21.2 is updated to say
  so. Reason it from measurement, not from the documentation.
  **Measured.** Settable: `SEM_FAILCRITICALERRORS`, `SEM_NOGPFAULTERRORBOX`, `SEM_NOOPENFILEERRORBOX`.
  `SEM_NOALIGNMENTFAULTEXCEPT` is **rejected** with `ERROR_INVALID_PARAMETER` -- loudly, not silently
  dropped, which is what the probe read every value back to distinguish. Two findings beyond the documented
  list: an invalid bit fails the **whole** call, installing none of the valid bits alongside it, so the
  declarable type must be unable to represent it rather than validating it at runtime; and M21.2 is
  narrowed rather than closed, since `SEM_NOGPFAULTERRORBOX` is settable and remains a real policy
  question. Recorded in
  [crates/windows-thread-ambient-sys/DESIGN-NOTES.md](crates/windows-thread-ambient-sys/DESIGN-NOTES.md).

- [x] **M22.3** -- Create the crate: `Cargo.toml`, workspace membership, `README.md`, a `CHANGELOG.md`
  baseline, a row in [PLANS.md](PLANS.md), and a crate `DESIGN-NOTES.md` recording the shape decisions
  before any of them are implemented -- the two-set decomposition (a capture set over capturable aspects,
  and declared fields that have nothing to collect and default to leaving the worker's value alone); the
  three-state per-aspect value that keeps *not captured* distinguishable from *captured and absent*, since
  both end with the worker on its own value and only one is deliberate; the default capture set as a
  **named constant** rather than a `Default` impl, because growing an implicit default silently changes
  behaviour for callers who never named it; the guard composition order; and the per-aspect restore policy.

- [x] **M22.4** -- Implement the thread error mode aspect: capture via `GetThreadErrorMode`, declaration of
  an explicit value, and scoped application restoring the worker's entry value on every path including
  unwind. This is the aspect that appears in **both** categories, and that is deliberate -- the facility
  captures the caller's value for diagnostics while declaring the forced dialog-suppressing bits, and
  keeping both available here is what stops this crate encoding one consumer's policy. Depends on M22.2 for
  the accepted bit set.

- [x] **M22.5** -- Implement the impersonation aspect by consuming
  [windows-impersonation-token-sys](crates/windows-impersonation-token-sys/DESIGN-NOTES.md) rather than
  reimplementing capture, transport, or restoration. Its restore failure is fail-fast and that semantics is
  inherited unchanged; note in the crate notes that its capture never yields an absent token, because it
  snapshots the process identity when the thread has none, so this aspect's *absent* state is unreachable
  by construction while the three-state shape is retained for uniformity.

- [x] **M22.6** -- Implement the TxF transaction aspect: capture the calling thread's current transaction,
  carry an owned duplicate so the value does not depend on the caller's handle outliving it, and apply it
  around the callback. Bind `ktmw32` lazily rather than linking it, so a consumer that never captures a
  transaction does not acquire a dependency nothing else in the workspace has. State the hazard the aspect
  cannot remove: the caller may commit or roll the transaction back while the worker is still inside it.

- [x] **M22.7** -- Implement the declared aspects -- WOW64 filesystem redirection, memory priority, and I/O
  priority. Each is unspecified by default, meaning the worker's own value is left untouched. Record why
  each is declared rather than captured, per aspect rather than as one blanket statement: redirection has
  no getter at all, memory priority is readable but is a policy choice rather than something a caller
  implicitly consents to remoting, and I/O priority has no documented getter and moves only in lockstep
  with CPU priority through background mode. Depends on M22.1 for the reclassification.

- [x] **M22.8** -- Give the aspect surface runnable examples, and compile the README as doctests. Added
  after M22.7 landed with **zero** doctests, which execution revealed to be a planning error rather than a
  deferral: M23.4 had scheduled all documentation at the end of the composite, so the aspects would have
  shipped a whole milestone with examples nothing compiled. Per this repository's rule that prose
  containing code must compile, the README carries
  `#[cfg(doctest)] #[doc = include_str!("../README.md")]`, so a contract change breaks the build instead of
  leaving the README teaching the old answer. Verify by sabotage that the README examples are genuinely
  executed rather than merely parsed. M23.4 retains the *composite's* documentation.
## M23 -- `windows-thread-ambient-sys`: the composite

- [x] **M23.1** -- Implement the capture set and its named default, covering only the capturable aspects.
  The default set is a named constant whose growth is a breaking change, so a caller who wants stability
  can name aspects explicitly and a caller who takes the default can see what it contains.

- [x] **M23.2** -- Implement composite capture, failing synchronously on the calling thread. A capture that
  cannot be performed is an admission failure, not a deferred one, and the error names which aspect failed.

- [x] **M23.3** -- Implement application as a composition of per-aspect guards, applied outermost-first and
  released in exact reverse, with the impersonation guard innermost because its window is narrowest and its
  restoration is the one that must not be delayed. Applying a subset must stay expressible, which is what
  the differing application windows require. Restore failure is fail-fast for impersonation, inherited
  rather than chosen; for the other aspects it is reported rather than fatal, and the report must reach the
  caller instead of being dropped on the floor.

- [x] **M23.4** -- Prove the *composite* across a real thread boundary rather than only in-process (the
  per-aspect cross-thread cases already landed with M22.4-M22.7, and the aspect documentation with M22.8):
  capture on
  one thread, apply on a thread-pool worker, and assert each aspect took effect there and was restored
  afterwards. Include the negative that motivates the whole crate -- an uncaptured aspect does **not**
  arrive on the worker -- since a test suite that only ever sees capture succeed cannot tell the two apart.
  Complete the API documentation, the README examples, and the changelog baseline.

- [x] **M23.5** -- Prove the composite against a **many-worker consumer's shape**, which is the audit's
  second purpose and was not discharged when M23 was closed. The in-repository consumers each apply a
  captured state on one worker at a time; Globazog takes one capture at `submit()` and shares it across up
  to 64 concurrent workers for the length of a traversal, and nothing currently tests that. Assert
  `AmbientState: Sync` -- it holds, but only `Send` was asserted, and `Send` alone would let this design
  pass its own suite and then fail to compile in the consumer that motivated it. Share one `Arc<AmbientState>`
  across concurrent pool callbacks, applying and restoring independently on each, and assert every worker
  saw the captured context and was left clean. Then document the two things a consumer of that shape must
  know and cannot currently learn from the crate: that applying once around a batch and applying per
  operation are both expressible and differ by a `SetThreadToken` per operation, so the granularity choice
  is theirs to make deliberately; and that an impersonation restore failure is fail-fast, which on a shared
  pool means a process abort rather than one failed operation.

- [x] **M23.6** -- Close mutation gaps with deterministic fault injection and exhaustive assertions. -> [completed 2026-09-01](COMPLETED-CHECKLIST.md#m236)

## M24 -- `windows-namespace-request-sys`: foundations

A sibling crate, not a layer above M22-M23: a request carries no ambient context, and a context is useful
to work that never opens a file. The submission site pairs them, which is what keeps both independently
reusable. This crate is the catalogue-plus-faithful-execution layer -- synchronous, testable with no ring,
pool, or async anywhere near it. The family grows by one entry per Win32 call.

**The round-one entry list is audited, not guessed.** It is the union of what three real consumers call:
[windows-file-watcher](crates/windows-file-watcher/src/directory.rs) and
[windows-file-enumeration-sys](crates/windows-file-enumeration-sys/src/native.rs) in this repository, and
`MikeGrier/Globazog-rs` at commit `55a0b1ae`.

| # | Entry | Needed by | Shape observed |
|---|---|---|---|
| 1 | `CreateFileW` | all three | `FILE_LIST_DIRECTORY`, share `R\|W\|D`, `OPEN_EXISTING`, `FILE_FLAG_BACKUP_SEMANTICS`; the watcher adds `FILE_FLAG_OVERLAPPED` (port branch), the other two omit it (unassociated branch) |
| 2 | `OpenFileById` | watcher | volume-hint handle + `FILE_ID_DESCRIPTOR`; no creation disposition |
| 3 | `FindFirstChangeNotificationW` | watcher | path, subtree flag, `FILE_NOTIFY_CHANGE_*` mask; handle-producing |
| 4 | `CloseHandle` and variant close routines | all three | `FindCloseChangeNotification` is **not** `CloseHandle` |
| 5 | `GetFileInformationByHandleEx` | all three | five classes: `FileBasicInfo`, `FileIdInfo`, `FileCaseSensitiveInfo`, `FileIdExtdDirectoryInfo`, `FileIdExtdDirectoryRestartInfo` |
| 6 | `GetFileInformationByHandle` (non-Ex) | watcher | `BY_HANDLE_FILE_INFORMATION`; a distinct call, not a class of entry 5 |
| 7 | `GetFinalPathNameByHandleW` | watcher directly, Globazog via `std::fs::canonicalize` | `VOLUME_NAME_DOS \| FILE_NAME_NORMALIZED` |
| 8 | `GetVolumeInformationByHandleW` | watcher | handle-based, not the path-based `GetVolumeInformationW` |
| 9 | `GetFullPathNameW` | enumeration | collapses `.`/`..` lexically, roots against process state |

Four audit findings that shape the milestones below, recorded because each contradicts an assumption the
first draft of this plan was written on.

**Five of the nine entries take a handle, not a path.** The first draft assumed a request owns everything
it names. Decided: a request **owns a duplicate**, taken with `DuplicateHandle` at capture, so it is
self-contained and cannot be left referencing a handle its originator has closed. That makes handle
ownership a shared primitive rather than an `hTemplateFile` detail.

**No consumer passes a security descriptor or a template file, and none creates a file.** Every audited
open is `OPEN_EXISTING` against a directory with a null `lpSecurityAttributes` and a null `hTemplateFile`.
Those parts of the `CreateFileW` entry are kept anyway: an entry that cannot express two of its own
parameters is a *narrowed* `CreateFileW`, and narrowing a platform entry to fit currently visible consumers
is the anti-pattern this repository's platform-integrity rule names. This is recorded so a later reader does
not mistake the absence of a consumer for an oversight.

**The strongest offload evidence is not an open.** Globazog's `QueryBuilder::submit()` calls
`std::fs::canonicalize` on the **caller's** thread, once per root -- a full `CreateFileW` plus
`GetFinalPathNameByHandleW` plus `CloseHandle` with unbounded latency on a network path -- and
`escapes_confinement()` repeats it per reparse-point candidate on a worker. Entry 7 is therefore
first-class, not second-tier.

**Globazog is a prospective consumer of the ambient crate, not evidence against it.** An earlier draft of
this section recorded that Globazog "uses no ambient thread state at all" and drew a structural conclusion
from it -- that the two crates are siblings rather than a stack. The observation is accurate about the code
as it stands and the inference from it was wrong: a consumer that is still synchronous-on-worker-threads
has not *needed* ambient state yet, which says nothing about whether it will. Globazog's own notes schedule
the async follow-up (`NtQueryDirectoryFile` plus IOCP), and that is exactly the point at which its work
moves onto pool workers and the caller's identity has to be marshaled to reach it. Every aspect this
workspace carries is plausibly live for it: impersonation for identity, the error mode because a traversal
is precisely what meets a dead network path or an empty removable drive on a shared pool thread, WOW64
redirection for a 32-bit host, and priority for a background scan. The sibling claim still stands, but on
its own footing -- a request needs no context and a context needs no request -- and not on this evidence.

**The audit had two purposes and only one was discharged.** Establishing the operation set is the first;
establishing that the *scenario* is adequately served is the second, and it was not answered. Globazog's
shape makes the scenario concrete and demanding in a way the in-repository consumers do not: one capture
taken at `submit()`, shared by up to 64 concurrent workers, applied repeatedly over a traversal that may
run for minutes. That imposes requirements no existing test covers, which are queued as M23.5 rather than
assumed:

- **One state, many workers, concurrently.** This needs `AmbientState` to be `Sync` and shareable through
  an `Arc`, not merely `Send`. It *is* `Sync`, verified, but only `Send` was ever asserted -- and `Send`
  alone would let a design pass its tests and then fail to compile in the consumer that motivated it.
- **Granularity is the consumer's choice and has a cost.** Applying the composite once around a batch of
  directories and applying it per open are both expressible, and they differ by a `SetThreadToken` per
  operation. Globazog's worker loop processes many directories per invocation, so the choice is real and
  the crate should say what it costs rather than leave it to be discovered.
- **Fail-fast has a blast radius on a shared pool.** An impersonation restore failure panics, and a
  panicking pool callback aborts the process. That is inherited and correct, but a consumer running 64
  concurrent impersonated workers should learn it from the documentation rather than from an incident.
- **Path resolution under a captured identity is still open.** Globazog resolves its roots on the
  *submitting* thread and opens them on workers. Under a token from another logon session, M20.1's
  session-relative drive letter hazard makes that a genuine divergence rather than a theoretical one, and
  the namespace-request crate inherits it.
- [x] **M24.1** -- Create the crate, with a `DESIGN-NOTES.md` recording the boundary decisions before
  implementation: a request excludes ambient context; a request captures parameters and performs the call
  faithfully but does not choose a delivery model, so the handle-destination fork stays out and an opened
  handle comes back plain and unassociated; the family grows one entry per Win32 call; and a request owns
  duplicates of any handle it names. Record the audited entry list above as the round-one scope, with its
  provenance, so a later reader can tell a deliberate omission from an unexamined one.

- [x] **M24.2** -- Implement owned handle references: duplicate at capture with `DuplicateHandle`, own the
  duplicate for the request's life, and close it with the request. This is the shared primitive behind both
  `hTemplateFile` and the five handle-taking entries, so it lands before any of them. Cover the case the
  audit makes unavoidable -- a source handle that is already closed, or is a pseudo-handle -- and decide
  whether duplication failure is a construction error (it is: capture fails on the caller's thread, where
  the caller can still do something about it).

  State plainly, in the type's own documentation, what a duplicate is and is not, because the distinction
  is the one a caller reasoning in terms of value semantics will get wrong: **a path is a value and is
  copied; a handle is a reference to a kernel object, and duplicating it shares that object rather than
  cloning it.** A request is therefore self-contained with respect to *lifetime* -- it cannot be left
  pointing at a closed handle -- and **not** isolated with respect to *state*. M26.1 measures where that
  distinction has teeth. One property this design depends on is measured there and must be asserted here
  too: closing the duplicate does **not** disturb the source, so a request owning a duplicate and dropping
  it cannot damage the handle its caller kept.

- [x] **M24.3** -- Capture the security attributes. A caller's descriptor may be **absolute**, holding raw
  pointers to owner SID, group SID, DACL and SACL that are quite possibly on the caller's stack, so capture  normalises to **self-relative** and owns the resulting contiguous blob. Two traps must be handled rather
  than discovered: a self-relative descriptor requires DWORD alignment, which a plain boxed byte slice does
  not guarantee; and *no descriptor*, *a descriptor with a NULL DACL*, and *a descriptor with an empty
  DACL* are three different security outcomes the type must keep distinct. Validate on capture, so an
  invalid descriptor fails at the caller rather than on the worker. The alignment requirement is not
  peculiar to descriptors -- M26.1 needs an 8-byte-aligned buffer for the same underlying reason -- so build
  it once as an owned aligned buffer primitive rather than twice.

- [x] **M24.4** -- Implement path preparation: resolve on the calling thread at construction, because the
  process current directory is mutable by any thread. Bind to the shipped precedent in
  [crates/windows-file-enumeration-sys/src/path.rs](crates/windows-file-enumeration-sys/src/path.rs)
  rather than writing a second path preparation. **That precedent's `prepare` is `pub(crate)`**, noticed
  while writing M24.1's design notes, so "bind to it" is not yet possible as written: it must be published
  from that crate or extracted to a shared one first. Duplicating it is the option this repository's
  mono-repo policy rejects -- fix the layer rather than work around it -- so decide which before
  implementing, and treat the decision as part of this item. The result inherits M20.1: until the session-independent
  path form is decided, a session-relative drive letter is a documented hazard on these types, and the
  documentation must say so rather than imply the resolution is complete.

  **Decided: copy it, temporarily and on the record.** Neither published option was taken. The enumeration
  crate is released and this one is not, so making it depend here would make it unpublishable, and this
  branch exists to reach publication with minimal impact on what already ships; extracting a third shared
  crate buys a new published member before any consumer justifies it. The copy is the duplicate-then-decide
  procedure working as intended -- the released path stays untouched while this one is proven -- and it is
  not permitted to become permanent by default: `path.rs` carries a provenance comment naming its source
  and commit, D-9 records the reasoning, and the merge-or-delete decision is scheduled as **M26+.3**, gated
  on this crate's first release.
- [x] **M24.5** -- Establish the faithful-execution contract that every entry then follows: an entry
  returns its result or the raw Win32 code **unaltered**, and `GetLastError` is captured before any
  restoration runs so nothing in between overwrites it. Preserving the code is a constraint from a real
  consumer rather than a stylistic choice -- `ERROR_FILE_NOT_FOUND` means a missing directory from an open,
  an empty directory from a first query, and a genuine failure from a later one, and only the consumer can
  disambiguate.

- [x] **M24.6** -- Test the foundations: security descriptors that are absolute, self-relative, null,
  empty-DACL, and invalid; handle duplication against a live handle, a closed handle, and a pseudo-handle;
  and the property that binds the whole crate together -- a captured request survives the caller dropping
  every input it was built from, including the source handle. Complete the API documentation and the
  changelog baseline.

  **Re-planned during execution.** The enumerated per-case tests were not deferred to this item: each
  landed with the item that introduced the behaviour, which is the sequencing the one-item-then-commit
  loop produces and is better than holding tests back to a trailing test item. What was genuinely left,
  and is what this item delivered, is the **composite** the per-module tests cannot show -- one value
  holding a prepared path, two captured handles, and captured security attributes, outliving every input
  at once and still working on a thread that saw none of them -- plus the crate example, the README
  example compiled as a doctest, and confirmation that the changelog baseline matches its siblings.
## M25 -- `windows-namespace-request-sys`: the handle-producing entries

Entries 1-4 of the audited list. Each depends on M24's foundations and on nothing else.

- [x] **M25.1** -- The `CreateFileW` entry, over the complete parameter set: path, desired access, share
  mode, security attributes, creation disposition, flags and attributes, and template file. It must express
  all three audited flag shapes, including the `FILE_FLAG_OVERLAPPED` split -- the watcher's open is
  destined for a completion port and the other two are not, and that difference is a request field rather
  than something the crate decides.

- [x] **M25.2** -- The `OpenFileById` entry. It is a second open primitive, not a `CreateFileW` variant: it
  takes a volume-hint handle and a `FILE_ID_DESCRIPTOR` and has no creation disposition. One entry per
  Win32 call means it is its own entry, and it is the first consumer of M24.2's owned handle on the input
  side.

- [x] **M25.3** -- The `FindFirstChangeNotificationW` entry. Path, subtree flag, and notification filter,
  producing a handle that is **not** closed with `CloseHandle`.

- [x] **M25.4** -- The close entries. `CloseHandle` belongs in the catalogue because it blocks on
  outstanding I/O and can block hard on a dead network path, which is the whole reason this facility
  exists. The audit shows a close entry cannot assume its routine: `FindCloseChangeNotification` closes
  M25.3's handle and `CloseHandle` is wrong for it. A handle therefore carries its close routine rather
  than the entry assuming one -- the same shape
  [windows-threadpool-sys](crates/windows-threadpool-sys/README.md) already needed for wait targets.

- [x] **M25.5** -- Prove the handle-producing entries against real directories, including the three flag
  shapes the audit found, the non-`CloseHandle` close routine, and a reopen-by-id that survives its source
  handle being closed first. Landed as an integration test (`tests/handle_entries/`) rather than more unit
  tests, because these cross a real filesystem boundary and chain entries together: the per-entry unit tests
  prove each entry against Windows in isolation, and only a composed test reaches the combination the audit
  called out -- a handle opened by one request becoming the *input* to a later one. Also covers the whole
  chain performed on a worker that saw none of its inputs, and many requests across concurrent workers,
  which is Globazog's shape.

- [x] **M25.6** -- Give the catalogue a **test seam**, so a consumer can exercise its own code against these
  entries without a filesystem. Every entry is a value whose `perform` is the single point where Win32 is
  touched, which is already the right shape -- what is missing is a trait over it, so a consumer's code can
  be generic over "a request that produces `T`" and take a fake in its tests. Two traits, not one, because
  the distinction is real rather than cosmetic: an open is a parameter set that may be performed repeatedly
  and takes `&self`, while a close is one-shot and consumes itself. Collapsing them would either make a
  close look repeatable or make every open look single-use. Prove the seam by writing a fake in a doctest --
  a seam nobody has substituted is a seam nobody knows works.

- [x] **M25.7** -- Give the public surface **runnable examples**. The crate currently has 6 doctests against
  roughly 128 public items, which is thin enough that a contract change could silently invalidate the
  documentation without breaking the build. Every public type gets a worked example, and every method whose
  correct use is not obvious from its signature gets one -- with priority on the ones a caller gets wrong:
  the three-way security and DACL distinctions, the two handle-failure conventions, what a duplicated handle
  does and does not share, and rearming a notification. These are compiled, so they cannot rot. This sets
  the standard M26's entries are then held to rather than being a one-off cleanup.

## M26 -- `windows-namespace-request-sys`: the query entries

Entries 5-9 of the audited list. All but the last take a handle, so all but the last depend on M24.2.

- [x] **M26.1** -- The `GetFileInformationByHandleEx` entry: one entry with the info class as a request
  field, per the one-entry-per-Win32-call rule. As a *marshaling* problem this is the easiest entry in the
  catalogue and should be built as such -- its inputs are a handle, a scalar class, and a buffer size, with
  no pointer into caller memory anywhere, so nothing needs normalising. An earlier draft of this item
  claimed the design problem was that the five audited classes have two result shapes (fixed-size
  out-params versus variable-length batches); that was wrong. This crate returns bytes and the unaltered
  outcome and does not parse, so both shapes collapse to one owned aligned buffer, and per-class parsing
  stays with the consumer that already owns it.

  The real difficulty is elsewhere, and it falls directly out of M24.2. **Measured**, not reasoned: an
  earlier draft asserted the following from the object-manager model, which is precisely the kind of claim
  this repository has been burned by. Measured on Windows 11 Enterprise 10.0.28000,
  `aarch64-pc-windows-msvc`, against a real directory with a deliberately small buffer so the cursor
  questions actually arise.

  | Question | Measured |
  |---|---|
  | Does a duplicated handle share the enumeration cursor? | **Yes** -- the source read `.`, `..`, `f00`; the duplicate returned `f01, f02, f03`, a clean continuation |
  | Control: do two separate opens share it? | **No** -- the second open restarted from `.`, so the probe can tell the two apart |
  | Does closing the duplicate disturb the source? | **No** -- the source continued correctly afterwards |
  | Does an interleaved `FileBasicInfo` disturb the cursor? | **No** |
  | Does an interleaved `FileIdInfo` disturb it? | **No** |
  | Does an interleaved non-Ex `GetFileInformationByHandle` disturb it? | **No** |
  | Does `FileBasicInfo` *on the duplicate* disturb the source's enumeration? | **No** |

  So the contract is **narrower** than the earlier draft claimed, and the difference matters. It is not
  that handle-taking entries are hazardous in general: **only the two directory-enumeration classes mutate
  the shared cursor**, and every other query is a pure read that composes freely with an enumeration in
  progress, on the same handle or on a duplicate. What the entry must state is therefore specific: a
  duplicate is not an independent enumeration, and an independent traversal needs a fresh open. This is
  also the one place the unresolved ordering question binds, since two *enumeration* requests against one
  handle are order-dependent in a way that no other pair of entries is.

  Two constraints that are not negotiable and are already solved in this repository, so bind to the
  precedent rather than re-deriving it. The buffer must be **8-byte aligned**: a `Vec<u8>` fails the very
  first query with `ERROR_NOACCESS`, which is why
  [crates/windows-file-enumeration-sys/src/buffer.rs](crates/windows-file-enumeration-sys/src/buffer.rs)
  backs its storage with `Vec<u64>`. And the call **reports no written length** -- a batch is walked by its
  own next-entry offsets -- so the completion returns the whole buffer and the consumer bounds its own
  reads, rather than the entry inventing a byte count it cannot know.

  Record that this entry needs **no ambient context**: access was checked at the open, which is exactly why
  the enumeration crate applies impersonation only around `CreateFileW`. It is the clearest case that a
  request and a context are paired at submission rather than fused.

  Finally, state the relationship to
  [windows-file-enumeration-sys](crates/windows-file-enumeration-sys/DESIGN-NOTES.md), because an entry
  covering the two directory classes otherwise looks like a second implementation of a shipped streaming
  engine. It is not: this entry is **single-shot** -- one call, one batch, and the *client* sequences the
  next, which is the one-entry-per-Win32-call rule applied literally -- while that crate is a streaming
  specialisation over the same shape, owning the cursor, the refill loop, the quanta, and backpressure. All
  five audited classes stay reachable here, because restricting them would narrow the entry for a
  no-consumer reason, which is the same move refused for `lpSecurityAttributes` in M24.3. The documentation
  must nonetheless point a consumer wanting *streaming* enumeration at that crate rather than leaving it to
  rebuild the loop from single-shot calls.

  Recorded because it was challenged directly and the challenge was reasonable: this entry needs almost no
  marshaling work, which invites the conclusion that it does not belong in the catalogue at all. Membership
  is decided by whether a blocking namespace call needs performing off the caller's thread, not by whether
  it is awkward to marshal -- the latter test would select for our implementation convenience rather than
  for consumer need. On the former test this is the most-called namespace operation across all three
  audited consumers, and the call whose lack of an overlapped form is why an unassociated handle is a
  first-class destination at all.

- [x] **M26.2** -- The `GetFileInformationByHandle` entry, returning `BY_HANDLE_FILE_INFORMATION`. It is a  distinct Win32 call rather than a class of M26.1, and the watcher uses it where the Ex form would not do.

- [x] **M26.3** -- The `GetFinalPathNameByHandleW` entry, including the flags the watcher relies on
  (`VOLUME_NAME_DOS | FILE_NAME_NORMALIZED`) and the grow-the-buffer retry the call requires. This is the
  entry the audit identified as having the strongest offload evidence, since Globazog performs it on its
  submitting thread today.

- [x] **M26.4** -- The `GetVolumeInformationByHandleW` entry, returning volume label, serial, and
  filesystem name. Handle-based; the path-based `GetVolumeInformationW` is deliberately not in round one
  because no audited consumer calls it.

- [x] **M26.5** -- The `GetFullPathNameW` entry. Does not verify its result: it collapses `.`/`..`
  lexically and roots most paths that are not fully qualified against process state -- the current
  directory, or for a drive-relative path naming another drive the entry recorded for that drive,
  while on the current drive that entry makes no difference -- and never expands a drive letter, so it
  does **not** close the session-relative hazard from M20.1, and its documentation must say which
  problem it solves and which it leaves standing.

  *(Corrected during the merge that brought PR #86 into this branch. This item was archived here while
  it still read "Lexical only", which is the claim that PR ran to twenty-four review rounds to remove:
  the call roots against process state, and for a drive-relative path naming another drive it checks
  that drive's `=X:` entry against the filesystem and rewrites a rejected one. Taking the archived copy
  unchanged would have reintroduced the false claim into the repository by way of the archive. See
  [crates/windows-namespace-request-sys/DESIGN-NOTES.md](crates/windows-namespace-request-sys/DESIGN-NOTES.md)
  -> `D-18`.)*

- [x] **M26.6** -- Acceptance, in **two** parts, because the audit had two purposes and checking only the
  first is how the coverage question got missed once already.

  *Operation coverage:* re-express each audited call site from the three consumers against the catalogue
  and confirm every parameter shape they use is reachable. This is the test that the entry list was derived
  from real consumers rather than from taste, and it must be run against all three -- the two
  in-repository crates and Globazog -- rather than the most convenient one.

  *Scenario coverage:* confirm the catalogue serves each consumer's actual **shape**, not just its call
  list. For Globazog specifically that means a request built on one thread and executed on another under a
  captured context, many such requests in flight across concurrent workers from one shared capture, and a
  handle opened by one request being carried into a later one -- which is where M24.2's owned duplicate and
  M26.1's shared enumeration cursor meet, and the one combination no single-entry test exercises. Record
  any gap as a defect rather than adjusting the scenario to fit what was built.

  Complete the API documentation and README examples.

## M27 -- `windows-platform-probes`: keep the measurements executable

Several decisions in this workspace rest on measurements of undocumented Windows behaviour. Recorded only
in prose, a measurement decays silently -- the claim stays in the design note while the platform, or our
reading of it, moves. This milestone gives them a durable home that an ordinary build keeps alive.

- [x] **M27.1** -- Create `windows-platform-probes` as an unpublished workspace member, with each probe's
  logic in a library function that **returns** its observation, so the binaries print it and the tests
  assert it from one implementation. Writing the check twice -- once to print, once to assert -- would make
  the test a check of the copy rather than of the platform, which is the restatement failure this
  repository has already paid for.

- [x] **M27.2** -- Adopt three tiers, because "run all the probes" is not a safe instruction: **asserted**
  (a real test), **ignored** (assertable but slow, heavy, or environment-dependent), and **binary only**
  (cannot be a test -- it hangs by design, mutates the process irreversibly, or needs privileges a test run
  must not assume). Every tier is compiled by an ordinary build, which is the floor. Record the tier of
  each probe and why, so a later contributor does not promote a hostile probe into the test path.

- [x] **M27.3** -- Migrate this session's measurements into the crate as asserted tests: the settable
  `SEM_` bit set, the whole-call failure an invalid bit causes, the independence of the thread error mode
  from the process error mode, and the four handle/cursor findings. Include the controls as their own
  assertions rather than as prose, and make a fixture that cannot exhibit the behaviour a **failure**
  rather than a silent pass. Verify the binding by sabotage -- change a fact and confirm a test actually
  fails -- since a guard only ever seen to pass is untested.

- [x] **M27.4** -- Migrate the nine earlier measurements' probes, which currently exist only in the
  git-ignored `.scratch/` directory and a previous session's private state, and are therefore one machine
  failure away from being lost. They are the evidence for the `IoRing` registration, thread-agnosticism,
  completion-port fork, token inheritance, `CancelSynchronousIo`, thread-pool growth, and device-map
  findings recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). Most belong in the ignored or binary-only tiers:
  one never returns by design, one moves 512 MiB, one spawns 512 threads, and one needs `subst` drives and
  a second logon session. Deliberately **not** done alongside M27.1-M27.3, which established the scheme on
  two cheap probes first.

  Landed as `worker_context` (asserted), `pool_growth`, `device_map` and `ioring` (ignored), and
  `cancel_io` (binary only). Two corrections were made in the move, recorded in
  [crates/windows-platform-probes/DESIGN-NOTES.md](crates/windows-platform-probes/DESIGN-NOTES.md): the
  device-map **control could never have passed** (it read a thread token the non-impersonating side does
  not have, so it always reported "same session"), and the `IoRing` registration probe must **not** use
  `windows-ioring-sys`, whose guard exists because of the very assumption being measured -- probing through
  it would confirm our own belief by consulting it. Calling Win32 directly also closes a standing gap: that
  crate recorded its replace-not-append assumption as explicitly *unverified*, and it is now measured and
  holds.

  **The completion-port fork is not migrated, and is not deferred for lack of need.** The original Probe D
  was superseded by its own corrected rewrite after the first version checked the wrong field and declared
  coexistence while its result code was `ERROR_INVALID_PARAMETER`. Re-establishing that measurement means
  re-deriving which of the two readings is right, which is measurement work rather than migration work.
  Queued as **M27.6** rather than folded in here, so it is scheduled instead of quietly dropped.

- [x] **M27.5** -- Re-run the probes on an **x64** host and record which findings are architecture-
  dependent. Every measurement in this workspace so far was taken on ARM64. This subsumes M19.5's narrower
  request for the thread-pool numbers, and the binaries exist precisely so this needs no re-derivation.

  **Done, by CI, on the first run.** The `platform probes (x64, ignored tier + magnitudes)` job in
  [.github/workflows/ci.yml](.github/workflows/ci.yml) runs on `windows-latest`, and PR #46's run of it
  answered the question. The full comparison is recorded in
  [crates/windows-platform-probes/DESIGN-NOTES.md](crates/windows-platform-probes/DESIGN-NOTES.md)
  -> `d-x64`.

  **No finding is architecture-dependent.** All fifteen qualitative facts held identically on x64: the
  settable `SEM_` bits and the whole-call failure an invalid one causes, the thread mode's independence
  from the process mode, the alignment bit's stickiness, all four handle/cursor findings, both worker
  ambient-state findings, the device-map change under impersonation, `IoRing` registration replacing the
  table, `IoRing` thread agnosticism, and both completion-port foreclosures. Nothing in this workspace's
  designs rests on an ARM64 peculiarity.

  Only the pool-growth **magnitudes** moved, and only in scale -- the burst-then-throttle shape is the
  same, which is precisely what the ignored tests assert and why they assert shape rather than numbers.
  Pinning the ARM64 interval would have failed here for no useful reason.

  Two things worth keeping. The warning that `IoRing` might report `Unavailable` on the runner was
  reasonable and turned out **wrong** -- `windows-latest` has a usable ring, so all four ring-dependent
  probes ran. And the one red job was a **test defect, not a finding**: a final-path test compared against
  `std::env::temp_dir()` as text, which the runner returns in 8.3 short form; it had passed locally only
  because that machine's user name is exactly eight characters. Classifying that correctly -- a red build
  that is not a platform difference -- is the job this comparison exists to do.

- [x] **M27.6** -- Migrate the completion-port fork measurement (Probe D): does associating a handle with
  an IOCP foreclose `IoRing` use of it? This is the evidence for `windows-namespace-request-sys` returning
  an opened handle **plain and unassociated**, so it is load-bearing for a shipped decision rather than a
  curiosity. It was split out of M27.4 because the original probe exists in two versions that disagree --
  the first declared coexistence while checking the wrong field, with a result code of
  `ERROR_INVALID_PARAMETER` and a zero byte count; the second checks the result and adds the negative
  control (the identical read on a non-associated handle) so a failure can be attributed to the
  association rather than to the probe. Migrating it therefore requires deciding which reading is correct,
  which is a fresh measurement rather than a port. Belongs in the ignored tier alongside the other
  `IoRing` probes, and must carry the negative control.

  **Settled: the corrected reading is right.** Measured on Windows 11 Enterprise 10.0.28000,
  `aarch64-pc-windows-msvc`: an unassociated handle reads fine (`0x00000000`, 4096 bytes, fill byte),
  and after `CreateIoCompletionPort` the same read is refused with `0x80070057`
  (`ERROR_INVALID_PARAMETER`) and zero bytes -- which is exactly the value the first version saw and
  misread as success. Both negative controls hold: the before-association read passes, and the associated
  handle still completes an overlapped read through its port, so it is the `IoRing` path specifically that
  is refused rather than the handle being broken. **`CreateThreadpoolIo` forecloses it the same way**,
  which matters more than the raw-IOCP case because it is the path this workspace actually uses.

  Recorded in [crates/windows-platform-probes/DESIGN-NOTES.md](crates/windows-platform-probes/DESIGN-NOTES.md)
  -> `d-completion-port`. Migrating it also found a latent fault in the shared `IoRing` fixture: its path
  was keyed only by process id and label, so concurrent tests collided and the resulting sharing violation
  **looked like the platform refusing something** -- the worst failure mode a probe can have. Fixtures are
  now unique per instance.

## M28 -- Probes that do not measure what they claim

Found by the M24-M27 code review. Every item here is a probe whose assertion passes without
establishing the fact it is cited for, which is the one failure mode this crate exists to prevent:
a vacuous probe does not merely fail to inform, it launders an unmeasured claim into
[crates/windows-platform-probes/DESIGN-NOTES.md](crates/windows-platform-probes/DESIGN-NOTES.md)
and from there into the design. Two defects of exactly this shape were already found and fixed
during M25 and M27, so treat this as a recurring class rather than a set of isolated slips.

- [x] **M28.1** -- Make `submitter_exited` an observation rather than a literal.
  `measure_thread_agnosticism` hard-codes `submitter_exited: true`
  ([ioring.rs](crates/windows-platform-probes/src/ioring.rs) `:470`), so the test's guard
  "the submitting thread must really be gone, or the probe measures nothing" is `assert!(true)`.
  The submitter calls `submit_and_wait(0)`, which returns once the SQEs are submitted, so a
  512-byte read of a cached temp file normally completes before the thread exits -- meaning the
  run is indistinguishable from one where the IRP genuinely outlived its thread, which is the
  claim the design rests on. Have the submitting thread record `ring.pop().is_none()` immediately
  before returning and report that, and force a genuinely pending IRP so the probe can fail.

- [x] **M28.2** -- Assert the premise in `measure_raise_while_saturated`. Its settle loop exits on
  saturation **or** timeout, and `before` is then whatever `started` happened to reach
  ([pool_growth.rs](crates/windows-platform-probes/src/pool_growth.rs) `:304`-`:317`). If the pool
  reached only 1 of `base_max`, the function times ordinary growth toward the *base* maximum and
  reports it as the effect of the raise; the `delay < 1s` assertion passes while measuring the
  wrong thing. Nothing asserts `before == base_max`. Add that assert, and return a value that
  distinguishes "the raise took effect after D" from "the settle window expired". Note the
  function's doc-comment already claims a panic that neither it nor `measure_growth` performs.

- [x] **M28.3** -- Make the identity-asymmetry test assert the asymmetry it names.
  `the_submitting_thread_and_its_worker_disagree_about_identity`
  ([tests.rs](crates/windows-platform-probes/src/tests.rs) `:202`-`:213`) binds
  `submitter_had_token` to `observed.is_unimpersonated()`, which is a property of the **worker**,
  not the submitter. The test therefore asserts byte-for-byte what
  `a_worker_does_not_inherit_an_impersonating_submitters_token` already asserts and checks no
  asymmetry at all. Have `observe_on_worker_while_impersonating` return both observations so the
  test can assert `submitter.has_thread_token && worker.is_unimpersonated()`.

- [x] **M28.4** -- Guard the separate-opens control against a vacuous fixture.
  `restarted()` is `next == source_first`, so if one `enumerate` call ever drained the directory
  both would be the full listing and the control would report "independent cursors" with no cursor
  ever left mid-directory ([handle_state.rs](crates/windows-platform-probes/src/handle_state.rs)
  `:308`-`:312`, test at `:101`). Unlike its sibling tests this path never calls `ground_truth`,
  which is where the `vacuous fixture` assert lives. It is non-degenerate today only by
  coincidence of `BUFFER_BYTES` and `FIXTURE_FILES`. This is the same 3-entry-fixture defect
  already fixed once on this branch, left on the control that gives the duplication finding its
  meaning.

## M29 -- Probe resource and isolation defects

Also from the M24-M27 review. These do not falsify a recorded claim on their own, but M29.1 can
manufacture a false negative that looks like the platform refusing something, and the rest are
handle or memory defects that CI now executes on every run since M27.5 added the ignored tier to
[.github/workflows/ci.yml](.github/workflows/ci.yml).

- [x] **M29.1** -- Stop the two `device_map` probes racing for one drive letter.
  `free_drive_letter` reads `GetLogicalDrives` and returns the first free letter with no
  reservation ([device_map.rs](crates/windows-platform-probes/src/device_map.rs) `:183`), and both
  ignored tests then `subst` the **same** target onto it. Under the parallel harness both can pick
  the same letter before either defines it; without `DDD_EXACT_MATCH_ON_REMOVE` the two targets
  stack on one letter and each removal pops one. Reported reproduced 7 runs in 8. The dangerous
  outcome is the second one -- `target: None`, a false negative on the fixture check caused by a
  sibling test rather than by the platform. Claim the letter atomically, use a distinct target per
  test, and pass `DDD_EXACT_MATCH_ON_REMOVE`.

- [x] **M29.2** -- Do not free an `OVERLAPPED` and its buffer while the I/O may be pending.
  `read_through_port` discards `ReadFile`'s return value; if the read goes pending and
  `GetQueuedCompletionStatus` times out, the stack `OVERLAPPED` and the heap buffer are destroyed
  and the port closed with the IRP outstanding, so the kernel later writes into freed memory
  ([completion_port.rs](crates/windows-platform-probes/src/completion_port.rs) `:251`-`:281`). The
  same shape exists on the `IoRing` timeout path
  ([ioring.rs](crates/windows-platform-probes/src/ioring.rs) `:324`-`:332`). `CancelIoEx` and
  drain the completion on the failure path so the buffer outlives the operation in every case,
  not only the happy one.

- [x] **M29.3** -- Bound the `IoRing` completion spin.
  `while completion.is_none() { completion = ring.pop(); }` has no deadline and no yield
  ([ioring.rs](crates/windows-platform-probes/src/ioring.rs) `:460`-`:463`), so a completion that
  never arrives spins a core forever. Every other wait in this crate is bounded. It is in the
  ignored tier that CI now runs, so it would burn the job's 15-minute ceiling rather than report a
  failure.

- [x] **M29.4** -- Release the workers on an unwind. `gate.open()` is a plain statement, so any
  panic between the first `submit()` and that call skips it
  ([pool_growth.rs](crates/windows-platform-probes/src/pool_growth.rs) `:201`-`:240` and
  `:286`-`:322`); `Drop for ThreadpoolWork` then waits rather than cancels, deadlocking the
  process permanently. Open the gate from a drop guard so unwinding releases the workers.

- [x] **M29.5** -- Close the `TP_IO` before its file.
  `CreateThreadpoolIo`'s `PTP_IO` is never passed to `CloseThreadpoolIo`, leaking one object per
  `measure()` call, and the file handle is closed while the still-live `TP_IO` references it --
  the inverse of the required teardown order
  ([completion_port.rs](crates/windows-platform-probes/src/completion_port.rs) `:218`-`:227`).
  Every other handle in this crate is closed exactly once via `Drop`. Wrap it in an RAII type that
  closes before the `File` drops.

## Moved 2026-09-09 21:34:00 -04:00 -- placement tool M1-M4 and M36: the measurement, the record, and the runner

From [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md), which remains open at M5 (distribution),
M5+ (withdrawn), M6 and M7. M36 is archived alongside M1-M4 despite its later number because it is
likewise complete; the numbering records authoring order, not execution order.

## M1: decisions that shape everything after

- [x] **PT-1.1** -- **Name the crate**, and record the reasoning. It measures what a producer/consumer
  handoff costs as a function of where the two threads run, which is broader than queues and narrower
  than "topology". Candidates to weigh rather than a foregone answer: `windows-placement-probe`,
  `windows-handoff-cost`, `windows-locality-report`. Check availability on crates.io before settling.
  **Named `windows-placement-probe`.** It says what the thing is (a probe, not a library to build on)
  and what it measures (placement), and it matches the `probe-` binary naming already in this
  workspace. `windows-handoff-cost` was rejected as too narrow -- the tool already reports topology and
  NUMA hops, which are not handoffs -- and `windows-locality-report` as understating that it *measures*
  rather than summarises. All five candidates were confirmed free on crates.io before choosing.
  Created as a workspace member with `publish = false`, because PT-5.3 has not decided crates.io yet
  and `false` is the setting that cannot publish something by accident.

- [x] **PT-1.2** -- **Decide what the submission record carries about the machine beyond the
  fingerprint**, specifically the CPU model name. The fingerprint deliberately omits model names
  because "a fingerprint that changes when the answer does not is a fingerprint nobody can compare" --
  correct for comparing placements, and a real loss when a stranger sends a result you cannot ask
  follow-up questions about.
  **This had to be settled before the first submission arrives, because the asymmetry is brutal.** A
  record cannot be regenerated: a field the tool did not collect is missing *permanently* from every
  result gathered before the omission was noticed, and the machines are other people's.
  Under-collecting is unrecoverable; over-collecting is a privacy cost that can at least be corrected
  going forward by collecting less.

  **Decided: collect the CPU model, the OS build, and a virtualisation hint. The canonical fingerprint
  string stays clean; all three live in the record beside it.**

  The reasoning, in the order it actually holds:
  - **A CPU model is not personal data.** It is a hardware characteristic shared by millions of
    machines. The things that would be sensitive -- hostname, user name, file paths, domain membership,
    serial numbers, installed software -- are not collected and must not be. That is the primary
    argument; it stands whether or not the model could be inferred.
  - **Withholding it gains nothing anyway**, because a detailed topology plus cache geometry narrows
    the field to a small class of parts. This is the supporting argument, and it is deliberately *not*
    treated as a principle: "it could be inferred, so collect it" would justify almost anything, and
    the test remains whether the field is sensitive on its own merits.

  **Two fields ride along by the same reasoning, and both are more explanatory for this dataset than
  the model is:**
  - **The OS build.** Placement cost is a scheduler behaviour, and the scheduler changes between
    Windows builds. Two results that disagree are otherwise indistinguishable from two builds
    disagreeing, and that is unrecoverable after the fact.
  - **A virtualisation hint.** This workspace has already established that **VM slices flatten
    topology** -- the EPYC slice reports one L3 domain and one NUMA node for silicon that has eight and
    two -- which is precisely why the interesting rows are unmeasured here. Being able to separate bare
    metal from VM submissions is therefore not incidental: it is the distinction that decides whether a
    submission can supply the missing rows at all. **Record it as a hint and label it as one**;
    hypervisor detection is not reliably decidable from user mode, and a field that overstates its
    confidence is worse than an absent one.

  **The runner can suppress the model**, with a flag, and the tool says so where it lists what it
  collects. Not because the field is sensitive in general, but because the one case where it might be
  is real and narrow -- an engineering sample or unreleased part would leak a name that is not yet
  public -- and because "here is what I collect, and you may turn this off" is a materially stronger
  thing to say to someone doing a favour than "trust me". The field is optional in the record, so a
  suppressed submission stays valid rather than becoming unparseable.
  **Suppression is recorded, not merely absent.** A field that is missing because the runner withheld
  it and a field that is missing because the host would not answer are different facts, and a
  collector that cannot tell them apart will eventually read one as the other -- the same reason an
  inexpressible placement is reported rather than skipped.

  **And the flag must not be oversold, which is the more important half.** For the engineering-sample
  case it addresses the *smaller* leak. A pre-release part is identified at least as well by its
  **topology** -- an unusual core count, a novel cache arrangement, an unreleased NUMA layout -- and
  the topology is the entire point of the submission, so it cannot be suppressed without making the
  record worthless. **The tool therefore cannot make an NDA-covered machine safe to submit from, and
  must not imply that it can.** Say so plainly in the README: if the hardware is confidential, the
  whole output describes it, and the right answer is not to send it. That is worth more to the
  audience most likely to own a multi-socket machine than any reassurance would be.

- [x] **PT-1.3** -- **Decide the fate of the three existing probe binaries** (`probe-topology`,
  `probe-core-affinity`, `probe-peer-index-cache`) once their modules move. Keeping them as thin
  wrappers preserves the internal workflow; deleting them removes a second way to run the same
  measurement and a second place for output to drift. **Do not decide by taste -- the risk being
  weighed is two renderings of one measurement disagreeing**, which this investigation has already hit
  three times.
  **Decided: keep them, and move the *rendering* into the library so there is only one of it.** The
  two stated worries turn out not to be in tension, because they are about different things. The
  engineer's -- that a combined binary accretes flags and modes until it is the grab-bag this crate was
  extracted from -- is about **entry points**. The drift worry is about **renderings**. Sharing the
  render code kills the drift risk outright, after which extra entry points cost nothing.
  So: the shared tool is **one binary, one run, one record**, because a stranger doing a favour must
  not be asked to run three things and collate them. The internal probes stay **separate and thin**,
  because running one measurement in isolation is the whole point of a development loop. Every binary
  becomes an entry point only; measurement *and* rendering live in the library and are called, never
  reimplemented. A binary that formats its own output is the defect, not a binary that exists.

## M1B: processor groups, before a large machine is ever offered

**Executes after M2, despite the number.** The code it changes lives in
[crates/windows-platform-probes](crates/windows-platform-probes) until the move, and doing this work in
its final home keeps the move a pure relocation with its provenance trail intact. The "before" in the
heading is about the *machine*, not about M2.

**This is the blocker that would waste the opportunity.** A large multi-socket host -- the kind that is
the entire point of this tool -- has more than 64 logical processors, so Windows presents it as
**multiple processor groups**, each numbering from zero.

- [x] **PT-1B.1** -- **Carry `(group, number)` as a processor's identity.** `ProcessorPlace` keys on a
  bare `u8` number and `places_from_topology` discards the group outright (`for (_group, number)`), so
  every group's processor 5 collides on one map key. **The result is not a crash.** Numbers stay below
  64 within a group, so `assert!(cpu < 64)` never fires: the tool runs, pins to whichever processor
  won the collision, and prints a confident placement table describing a topology it silently
  collapsed. That is the same defect class as the omitted SMT row, on the machine we would get one
  attempt at.

- [x] **PT-1B.2** -- **Pin with `SetThreadGroupAffinity`.** `SetThreadAffinityMask` takes a mask
  within the caller's current group and cannot express a processor in another one, so it is not a
  matter of widening the mask. Keep the existing failure discipline: pinning that does not land must
  abort the run rather than fall back to an unpinned measurement.

- [x] **PT-1B.3** -- **Verify against a synthetic multi-group topology**, since no host here has more
  than one group. `places_from_topology` is a pure conversion and already testable; a fixture with two
  groups whose numbers overlap must produce distinct processors, and the sabotage is to key on the
  number alone and watch the count halve.
  **Nine tests added, and they found two real defects rather than confirming the change.** `classify`
  compared `core` without comparing `group`, so two processors in different groups whose core ids
  collided were reported as **SMT siblings** -- attributing a shared L1 that cannot exist, since a core
  cannot span a group. And the fallback core id was derived from the number alone, which is what made
  those collisions possible.
  **The planned sabotage could not be performed, which is the strongest available result.** Keying the
  conversion on the number alone no longer *compiles*: the maps are keyed `(u16, u8)`, so the collapse
  is unrepresentable rather than merely tested against. The sabotage that does compile -- dropping the
  group comparison from `classify` -- was performed and is caught.
  **One test asserted a promise the code never made** and was rewritten rather than the code bent to
  fit: `representative_pairs` returns one pair per placement *category*, and "in a different group" is
  not a category, so requiring a pair from every group was wrong. Its fixture also gave both groups the
  same cache-domain ids, describing a cache shared across groups, which no machine does.

- [x] **PT-1B.4** -- **Refuse loudly if groups are present and unsupported.** Whatever remains
  unimplemented when a large machine is offered, the tool must say so and stop. A refusal costs one
  message; a collapsed topology costs a wrong answer nobody can detect from the output, on hardware
  that is not coming back.

## M1C: direction and memory placement, before a NUMA machine is spent

**Raised by the engineer asking why the hop count was the edge count rather than twice it.** It should
be twice it, and answering that exposed a second defect underneath.

- [x] **M1C.1** -- **Measure both directions of a node pair.** `node_pairs` is undirected, with the
  reasoning that `0 -> 1` and `1 -> 0` "traverse the same link". That conflates the *link*, which is
  symmetric, with the *workload over it*, which is not: the producer **writes** slots and
  release-stores `tail`, the consumer **reads** slots and release-stores `head`, and a remote write
  needs exclusive ownership and invalidation where a remote read does not. Swapping the ends is a
  different measurement, not a repeat.
  Doubles the hop count, so state the cost plainly: `n*(n-1)` rather than `n*(n-1)/2`. On a four-node
  host that is 12 hops instead of 6, and the runtime estimate must follow.
  **Keep the two directions distinguishable in the record.** Reporting a mean of them would destroy
  exactly the asymmetry this item exists to measure.

- [x] **M1C.2** -- **Control and record which node the ring's memory is on.** `Ring::new` runs on the
  calling thread, which is never pinned, so under first-touch the ring lands on whatever node the
  *orchestrating* thread happened to occupy -- possibly neither the producer's nor the consumer's.
  **On a multi-socket machine there are three positions, not two**, and the third is currently
  uncontrolled and unrecorded. Two runs could differ solely because the main thread migrated, with
  nothing in the output to say so.
  This is not a refinement; it is what makes a NUMA number mean anything. A hop measured with the
  memory on an unknown third node is not a measurement of that hop.
  **Decided: measure both endpoints as separate rows.** The memory goes on the producer's node in one
  row and the consumer's in another, and **the memory node is recorded beside the two processor
  nodes** in every row.
  This measures remote-write and remote-read cost independently, which is the pair of quantities the
  asymmetry in M1C.1 is actually about: with memory on the producer's node the producer writes locally
  and the consumer reads remotely, and swapping the memory reverses exactly that.

  **The cost, stated plainly.** Four configurations per undirected edge -- two directions times two
  memory placements -- so `2*n*(n-1)` hop measurements rather than today's `n*(n-1)/2`. On a four-node
  host that is 24 rather than 6, and at two strategies and three repetitions it is 144 timed handoffs
  for the hops alone. Under two minutes at the worst per-item cost measured so far, which is
  affordable for hardware this scarce. **PT-4.2's estimate must be updated with it**, or the tool will
  under-promise the wait on precisely the machines that take longest.

  **The design carries its own consistency check, which is worth keeping rather than optimising
  away.** Of the four configurations per edge, two are "producer-local" and two are "consumer-local",
  differing only in which physical node each role sits on. On a symmetric interconnect each pair
  should agree; **if they disagree, the interconnect is asymmetric, and that is a finding** rather
  than noise. Averaging the pairs, or measuring only one of each, would discard it.

- [x] **M1C.3** -- **Say what the placement label means once direction exists.** A row currently reads
  as a pair of positions; it must read as producer-here, consumer-there, memory-somewhere. The
  existing `Placement` names are direction-free and will quietly under-describe a directed run, which
  is the "table with right labels and wrong pairs" failure in a new place.

## M2: the move

- [x] **PT-2.1** -- Move `fingerprint`, `core_affinity` and `peer_index_cache` into the new crate, and
  make `windows-platform-probes` depend on it. This inverts today's direction deliberately: the
  published crate owns the measurement, the internal grab-bag borrows it. A **pure relocation** with
  the provenance trail the repository requires for a split -- commit trailers and per-file headers --
  because these modules carry a session's worth of hard-won reasoning in their comments and blame must
  survive.

- [x] **PT-2.2** -- Keep `queue_contention` and every unrelated probe where they are. The new crate is
  not a home for "measurement code in general"; it is one tool with one question, and admitting a
  second unrelated probe is how it becomes the grab-bag it was extracted from.

- [x] **PT-2.3** -- Verify the move changed no behaviour: the three probe binaries (or their
  replacements per PT-1.3) produce the same numbers on this host as recorded in
  [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M-inf.4, and the full sabotage set still fails
  where it should.
  **Verified three ways.** Git recorded all five files as **100% renames**, so this was a pure
  relocation and `--follow` and blame both carry through without the provenance headers a *split*
  would need -- a split copies and leaves the source behind, a move does not. Test counts add up
  exactly: 58 in the new crate plus 25 in the old is the 83 that existed before. And
  `probe-core-affinity` reproduces its pre-move results (siblings 1.8x WINS at batch depth ~85,
  cross-cache 0.54x LOSES at ~1.9), which is the check that matters, because compiling proves the
  names resolved and nothing more.
  **One defect surfaced, unrelated to the move and pre-existing:** `queue_contention` still imported
  `windows_waitable_queues::mpsc`, stale since the `slotwise_mpsc` rename. It went unnoticed because
  nothing had rebuilt that crate since, and the move is what forced the rebuild. Fixed here rather
  than left for the release.

## M3: the submission record

Ordered so each item's prerequisites land first: the two identity fields are decided before the record
that carries them is written.

- [x] **PT-3.1** -- **A linearly increasing integer schema version that cannot silently drift, guarded
  by an archived schema rather than a hash.** The counter itself is easy for a consumer to compare
  (`schema >= 2`); the hazard is forgetting to bump it when the record's shape changes, which no amount
  of care reliably prevents. So derive rather than restate, per this repository's own rule -- but
  derive into something that survives.
  **A hash was considered first and rejected, because it does not survive its own history.** With a
  table of `version -> hash`, only the *current* version's hash can ever be recomputed; every earlier
  row is a frozen constant nobody can verify. The hash function then becomes an unversioned contract --
  change the traversal, the digest, or how key paths are canonicalised, and every historical row
  silently becomes wrong, with nothing to detect it. A digest is also opaque: it reports *that* the
  shape moved and never *what* moved, so a review cannot see whether a change was additive or breaking.
  **Archive the shape itself.** One golden file per schema version, listing the record's key paths
  (sorted, recursively) as text. A test generates the current shape and asserts it equals the golden
  for the current `SCHEMA_VERSION`; a change fails the test and the diff *shows what changed*. Bumping
  means adding the next golden, deliberately.
  This buys three things a hash cannot: a stored submission can be **validated against the schema it
  declares**, years later; the version-to-version diff is **reviewable**; and there is **no hash
  function to keep stable**, so no way for history to rot.
  **Golden files are append-only and a published version is never redefined** -- the same discipline as
  [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Once a record exists in the wild claiming schema N,
  N's meaning is fixed, because the record cannot be regenerated. Verify by sabotage: add a field,
  confirm the test fails and names the difference; bump, add the golden, confirm it passes.

- [x] **PT-3.2** -- **Stamp the exact build, and say loudly when it is not an official one.** The
  record carries the git commit, whether the working tree was dirty when it was built, the crate
  version, and whether it came from CI or a local build.
  **This is the same problem as `Provenance` one layer up, and takes the same shape**: an official
  CI-built binary from a clean tree is the trusted case, and everything else -- a local build, a dirty
  tree, an unknown commit -- must be visibly marked so a result that arrives from one is not silently
  pooled with the rest. Default to the untrusted reading when the answer cannot be established, for
  the same reason `Provenance::Synthetic` is `Default`: forgetting must be safe.
  A [build.rs](crates/windows-placement-probe/build.rs) reads the commit from an environment variable when CI sets one, falls back to `git`
  when there is a repository, and records *unknown* otherwise -- which is exactly what a `cargo
  install` from a crates.io tarball will produce, and is the honest answer there.

- [x] **PT-3.3** -- Emit **one** machine-readable record per run, carrying: the schema version
  (PT-3.1), the build identity (PT-3.2), the topology **provenance**, a UTC timestamp, the host
  fingerprint, every placement measurement, and every node-hop measurement.
  **Build identity is the load-bearing field.** Results will arrive over months from different builds,
  and a measurement that does not say which build produced it is an unlabelled number -- the exact
  failure this workspace spent [crates/windows-topology-sys/DESIGN-NOTES.md](crates/windows-topology-sys/DESIGN-NOTES.md)
  `D-12` fixing one layer down. There is currently **no** version stamped in any probe output.

- [x] **PT-3.4** -- Keep the human-readable report as well, and derive both from the same measured
  values so they cannot disagree. The reader running the tool should be able to see, in prose, the
  same conclusion the record encodes -- otherwise nobody notices when a run is nonsense.

- [x] **PT-3.5** -- **The terminal output is the submission.** Collection happens by asking people to
  paste a run into a GitHub Discussions thread on this repository, so the paste is the channel and the
  whole record must survive it.
  **This reverses this item's original reasoning, and the reversal is the point.** It previously said
  to write a file *because* copying terminal output invites truncated and reflowed submissions. That
  risk is real and does not go away by choosing a different channel -- it has to be *mitigated* rather
  than avoided:
  - **Everything needed is on screen.** The record prints to stdout, not only to a file. A submission
    that requires the sender to find and attach a file will sometimes arrive without it.
  - **A self-check the reader can run.** A short checksum over the record, printed beside it, so a
    truncated or reflowed paste is *detectable* rather than silently half-ingested. This is the same
    principle as the schema golden: detect corruption instead of trusting the channel.
  - **Paste-safe formatting.** GitHub renders Discussions as markdown, so the output must survive a
    fenced code block and must not depend on colour, cursor control, or overlong lines that wrap.
  - **Tell the runner exactly what to do**, in the output itself: which thread, and to paste inside a
    fenced block. An instruction that lives only in a README is an instruction half of them will not
    have read.
  **The target is select-all, copy, paste, done.** Every extra step is a submission that does not
  arrive, so the tool emits its own markdown fences: a runner who has never thought about markdown
  pastes the whole thing and it renders as a code block anyway. Instructions caught inside the fence
  are trivial noise next to a paste that renders as mangled prose.
  A file is still written, because it costs nothing and someone will prefer to attach one -- but it is
  a backup, never a required step, and the run must be complete and submittable without it.

- [x] **PT-3.6** -- Read the three machine-description fields PT-1.2 settled, each of which needs a
  source this crate does not currently use. **Every one of them is optional in the record**, so a host
  that will not answer produces a record missing a field rather than a failed run or a fabricated
  value.
  - **CPU model** -- the registry's `ProcessorNameString` under
    `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0` is the pragmatic source and works on both
    x64 and ARM64, unlike the CPUID brand string.
  - **OS build** -- the reported version must be the real one. The Win32 compatibility shims lie to
    unmanifested processes about the major version, so verify against a known build rather than
    trusting the first API that returns a number.
  - **Virtualisation hint** -- record confidence honestly. There is no user-mode call that decides
    this, so whatever signal is used, the field says "hint" and a negative means *not detected* rather
    than *bare metal*.

## M4: the runner's experience, and their trust

- [x] **PT-4.1** -- **One entry point.** A single binary that runs everything and produces one record.
  "Run these three and send me all three outputs" is friction for someone doing a favour, and invites
  partial submissions that cannot be compared.

- [x] **PT-4.2** -- **State the runtime before doing the work**, from the discovered topology rather
  than a guess: the hop matrix alone is `n*(n-1)/2` hops times two strategies times three repetitions
  times two million items, on top of the placements. On a four-node machine that is a materially
  longer run than on this one, and the person deserves to know before it starts.

- [x] **PT-4.6** -- **Set up the Discussions thread people paste into, and link it from the tool.**
  The tool's output names where to send a result, so that destination has to exist before the tool
  ships, not after -- an instruction pointing at a thread that is not there is worse than no
  instruction. Pin it, and state in the first post what is collected and what a submission is used
  for, so a reader who arrives from a search rather than from the README still sees it.
  **Done: [discussion 55](https://github.com/MikeGrier/windows-threadpool-sys/discussions/55), "Please
  share data from `windows-placement-probe`".** The tool points at that thread rather than at the
  discussions index, so a runner lands on the reply box instead of a list they have to search -- a
  link that costs an extra navigation is one more place a submission stops. A test asserts the URL
  still ends in a discussion number, which catches the plausible later mistake of trimming it back to
  the index during a tidy-up.

- [x] **PT-4.3** -- **Say exactly what is collected and what is not**, in the tool's own output and in
  its README, and make it verifiable by reading the record. Collected, per PT-1.2: core/cache/NUMA
  shape, timings, CPU model, OS build, and the virtualisation hint. **Not** collected: hostname, user
  name, file paths, environment variables, serial numbers, or anything about installed software --
  and that list is a commitment, not a description of the current implementation. **The tool makes no
  network connections**; the person sends the file themselves, deliberately. Mention the model
  suppression flag here, where someone deciding whether to run it will actually see it -- alongside
  the honest limit from PT-1.2, that the flag does not make confidential hardware safe to submit,
  because the topology describes the part regardless.

- [x] **PT-4.4** -- Pin the thread-pinning failure behaviour for a stranger's machine. It currently
  panics, which is right for us (a silently unpinned thread measures the scheduler, not the placement)
  but reads as a crash to someone doing a favour. It must fail with an explanation of what could not
  be pinned and why the run cannot continue honestly -- **and must not fall back to an unpinned
  measurement**, which would produce a plausible number that means nothing.

- [x] **PT-4.5** -- **Let the runner see everything before sending it, and decide with the real values
  rather than a promise.** This is a stronger privacy property than any suppression flag, and cheaper:
  the record is a text file, so the honest instruction is "open it and read it -- if you are not happy
  with something in there, do not send it."
  Two things make that instruction usable rather than theatre:
  - **A fast preview of the machine-description fields**, available without running the measurement.
    The full run takes minutes and grows with node count; nobody should have to spend that to discover
    what the tool would learn about their machine. Someone can then look, decide, and only then commit
    to the run.
  - **A record a human can actually read** -- field names that mean something without a schema in hand,
    and no opaque blobs. A file that must be decoded to be checked cannot honestly be described as
    inspectable.

- [x] **PT-4.7** -- **Lay the JSON out for a reader rather than a parser.** `to_string_pretty` gives
  every array element its own line, so eight cache domains cost eight lines saying `2` and a
  sixty-four-node host would spend sixty-four lines listing its nodes one integer at a time -- on
  precisely the machine whose submission matters most. Arrays holding no object now collapse onto one
  line, or fill lines up to a width budget, while objects still expand one field per line.
  **Field order is part of the contract, not incidental.** The first draft laid out a
  `serde_json::Value`, whose object is a `BTreeMap`, and every test still passed while the output
  quietly sorted `build` above `schema_version` and made each measurement row open with
  `consumer_batch`. Caught by reading a run, not by the suite. Order is now preserved by walking an
  order-preserving tree; `serde_json`'s `preserve_order` feature is deliberately **not** used, because
  cargo unifies features and four other crates in this workspace share `serde_json`.

## M36 -- Redact the secondary metadata by default

- [x] **M36.1** -- **Floor the record's timestamp to the minute, in UTC.** Done 2026-09-04. A
  second-precision stamp links two submissions from one host to each other even after every
  identifying field is withheld, and nothing in the analysis needs finer -- these measure a machine's
  shape, not an ordering of events. UTC with no local offset, because an offset narrows the submitter
  to a band of longitudes for no gain. `recorded_at_subsecond_millis` is untouched: it is
  `serde(skip)` and exists only so two runs in one second get distinct file names.

- [x] **M36.2** -- **Redact the secondary metadata by default, with an opt-in to include it.**
  Done 2026-09-04, with a single `--include-metadata` as recommended; `--no-cpu-model` survives as a
  subtraction from it, because the confidential-part case it was built for is not covered by the
  general opt-in and passing it alone is redundant rather than wrong. Suppression is recorded for
  every newly redactable field: `os_build_suppressed` and `recorded_at_suppressed` beside their
  `Option`s, and a `VirtualisationHint::Suppressed` variant rather than a flag, since that enum's
  other variants are all claims about what was observed. The backup file's name drops the stamp with
  the record, so the withheld minute cannot escape through a file a runner attaches. See
  [DESIGN-NOTES.md](crates/windows-placement-probe/DESIGN-NOTES.md) -> "The measurement is not
  redactable; the context is, and is withheld by default".
  Engineer's decision, 2026-09-04. The secondary metadata is the timestamp, the OS build, and the
  hypervisor name/hint -- everything in `MachineDescription` and the `recorded_at*` fields that is
  *context* rather than *measurement*. The topology is excluded from this by construction: it is the
  measurement, and the README already says so plainly.
  **Default flips to redacted.** `--no-cpu-model` becomes one case of a general rule rather than the
  only switch. Decide whether the opt-in is one flag or per-field; a single `--include-metadata` is
  the smaller surface and is the recommendation unless a per-field need appears.
  **Suppression must stay distinguishable from absence**, which the existing `model_suppressed` flag
  already does for the model: a field withheld by the runner and a field the host would not answer are
  different facts, and a collector that cannot tell them apart will read one as the other. Every newly
  redactable field needs the same treatment.
  **No `SCHEMA_VERSION` bump**: the freeze starts at the first release and this crate has not had one.

- [x] **M36.3** -- **Say in the README what redaction costs.** Done 2026-09-04, as a
  "What redaction costs" section. Built on the asymmetry `PT-1.2` already established -- withheld
  context cannot be recovered later, while over-collection can be corrected going forward -- then
  what each of the four fields buys, ordered by explanatory value rather than by sensitivity, with
  the minute named as the weakest of them. Two guards against mis-reading the new default: redaction
  does not make a submitter anonymous, because the topology is always sent and is the most
  identifying thing in the record; and a redacted submission is still a good submission, because
  sending nothing is by far the worse outcome. There is real value in correlating
  metadata anomalies with specific platform versions -- a defect that shows up only on one OS build,
  or only under one hypervisor, is exactly what the secondary metadata is for. A reader choosing to
  include it should understand they are helping, and a reader choosing not to should understand what
  they are withholding. State the trade rather than presenting redaction as free.

- [x] **M36.4** -- **On `Coherence::Disagreed`, ask for the unredacted record privately.** Done
  2026-09-04. **The dependency below was mis-stated and is corrected here**: `Coherence` was *not*
  reachable from the record. `topology_provenance` is carried and `Fingerprint` is built from the
  topology, but the fingerprint carries only the provenance, so the record had no way to know its
  two sources had disagreed. The record gained `topology_coherence`, carrying the whole `Coherence`
  including the processor lists -- a boolean would have made the ask hollow, since the record a
  maintainer is offered has to contain what they would look at. It is a field of the *record* and
  deliberately not of the `Fingerprint`, which is compared for equality to catch a spliced record
  and would then discard a good measurement over a difference in no shape at all.
  The wording is informative rather than coercive, per the engineer's direction: it reports what was
  detected, says the measurements are unaffected, names both possible causes -- inconsistent
  platform metadata *or* a defect in this tool -- as undecidable from the runner's machine, offers a
  way to help, and closes with "None of that is required." A test asserts the release is present and
  that no pressure word appears. See
  [DESIGN-NOTES.md](crates/windows-placement-probe/DESIGN-NOTES.md) -> "A disagreement is reported
  where it happens, and the ask attached to it is an offer".
  The report
  emits extra text when the topology's two sources disagreed past the retry: say that the metadata was
  inconsistent, and ask the runner to contact the `windows-threadpool-sys` maintainers through the
  discussions or issues boards and share an **unredacted** probe file **privately**, so the
  inconsistency can be verified -- or the probe fixed -- and a bug logged with Windows.
  **This is the point of the whole design.** Redaction is the default because most records do not need
  the context; the one case where the context matters most is a disagreement, which
  [D-17](crates/windows-topology-sys/DESIGN-NOTES.md#d-17) attributes to prerelease hardware,
  defective firmware tables, or a feature landing in one enumeration before the other -- the
  bug-worthy cases. So the request is made exactly there, and privately, rather than by collecting
  everything from everyone against the possibility.
  Depends on M36.2 (there must be something to un-redact) and on `Coherence` being reachable from
  the record, which it is: `topology_provenance` is already carried, and `Fingerprint` is built from
  the topology.

## Moved 2026-09-09 21:35:18 -04:00 -- io-domains M30: the queue crate's name, skeleton and SPSC shape

From [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md), which remains open at M31 (one item), M32,
M33+ and M-inf. M31's seven finished items stay in place: they belong to a group that is still open,
and converting them to stubs is queued separately as M34.3 in [CHECKLIST.md](CHECKLIST.md).

## M30 -- The queue crate: name, skeleton, and the SPSC shape

- [x] **M30.1** -- Decide the crate's name and record why, **before** anything depends on it, because
  renaming a crate that has dependents is churn this repository avoids. Two things to settle together:
  whether the `-sys` suffix applies (every existing `windows-*-sys` crate is thin-over-Win32, and this is
  a data structure with an opinion, so it probably does not), and the name of the domain runtime crate
  that will sit above it, since the pair should read as a pair. Candidates raised: `windows-io-queue`,
  `windows-signalled-queue`, `windows-queue`. Record the decision in [DESIGN-NOTES.md](DESIGN-NOTES.md)
  so the reasoning survives the choice.
  **Decided: `windows-waitable-queues`**, no `-sys` suffix, recorded in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#the-waitable-queues-crate-is-named-plural-and-carries-no-sys-suffix).
  The engineer proposed the plural and it is right for a reason stronger than taste:
  [windows-platform-probes](crates/windows-platform-probes/README.md) is already plural, so the workspace
  distinguishes singular-for-one-facility from plural-for-a-collection-of-peers, and this is the second
  kind. **`windows-io-queue`, floated during the same discussion, was rejected** -- the queues have
  nothing to do with I/O, and naming a general facility after its first consumer is the mistake
  `windows-topology-sys` avoided. What unifies them is waitability, a word this workspace already owns
  through `WaitableHandle`.
  **One consequence accepted deliberately:** the plural forbids a bare `Queue` type, since a crate named
  "queues" exporting one would claim a primacy the name denies. Every type is specifically named and a
  consumer must say which it wants.
  **The runtime crate's name is deliberately NOT fixed here**, which narrows this item as written. The
  churn argument applies to a crate with dependents, and M30.2 creates the queue crate immediately while
  the runtime does not exist until M33+. The rule is recorded instead -- the pair should read as a pair,
  and the runtime's name may carry `io` because that crate genuinely is about I/O.

- [x] **M30.2** -- Create the crate with `publish = true` (the engineer's decision: this is general-purpose
  and worth publishing, unlike `windows-guard-alloc`), and write its [DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md) with the decisions
  the session already reached: the shape menu and which shapes ship now, the
  concrete-types-plus-optional-trait rule, the overflow policy, and the doorbell invariant. This is the
  Tier-1 transcription of Tier-3 session content -- design notes are not a work queue, so a decision that
  lives only in the session record is orphaned.

  **Settle one question here that M30.3 then depends on:** whether `WaitableQueue` is a single
  consumer-side trait, or a producer trait and a consumer trait. Waitability lives on the consumer -- it
  waits, while the producer merely rings -- so a single trait would be consumer-side and the producer's
  contract would go unnamed. Decide rather than defer, because M30.3 writes the first signatures against
  whichever answer this gives.
  **Answered, and the question turned out to be the wrong one.** The engineer's observation that the
  shapes would be "sliced and diced by various traits as we go along" is right, and following it shows a
  single `WaitableQueue` trait -- one *or* a producer/consumer pair -- is not merely inelegant but
  **unimplementable by the shapes that are planned**: a poll-only queue has no doorbell to return, and an
  unbounded one has no capacity to report. So the answer is **narrow capability traits** on the
  `std::io` model (`Read`/`Write`/`Seek`, not one `Io`), each naming one capability, with a shape
  implementing the subset it genuinely has. Recorded as [D-2](crates/windows-waitable-queues/DESIGN-NOTES.md#d-2)
  with the anticipated set.
  **And no trait ships until a second implementation exists to validate it** ([D-3](crates/windows-waitable-queues/DESIGN-NOTES.md#d-3)):
  a trait written against one type designs in a vacuum, since every signature that type happens to have
  looks like a requirement. The trait *shape* is fixed now because it constrains M30.3; the traits
  themselves land with M31.1.
  Crate created with [DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md) (D-1..D-8),
  [README.md](crates/windows-waitable-queues/README.md), [PLANS.md](PLANS.md) pointing back at this file, and
  registration in the workspace members, [release-please-config.json](release-please-config.json), and
  [.release-please-manifest.json](.release-please-manifest.json)
  -- the last two because `publish = true` makes it release-managed, and omitting them would have left it
  silently unreleasable.
  **One earlier position reversed with its reason recorded** ([D-7](crates/windows-waitable-queues/DESIGN-NOTES.md#d-7)):
  shapes are plain modules, not Cargo features. Two features are four configurations against a
  `feature-matrix` CI job that would have to grow, and the benefit is one dead-code elimination already
  provides.

- [x] **M30.3** -- The SPSC bounded ring, with no doorbell and no Win32 at all: a pure data structure with
  acquire/release head and tail and no CAS on either side. It is the CQ direction (R1), and it is first
  because everything harder is a variation on it. Tests are ordinary fast unit tests -- capacity edges,
  wraparound, full and empty, and that a `pop` never observes a partially written `T`.

  **This item sets the shape every later queue must match**, so it is where the trait-compatibility
  constraint binds: split producer and consumer handles, with cardinality carried by whether each is
  `Clone` (see
  [DESIGN-NOTES.md](DESIGN-NOTES.md#the-waitable-queues-crate-is-named-plural-and-carries-no-sys-suffix)).
  Getting this wrong is not a local mistake -- if the first shape ships a signature the second cannot
  match, the `WaitableQueue` trait becomes a breaking change to one of them rather than an addition.
  Verify it the cheap way: write the trait's method signatures down as a comment before writing the
  type, and confirm the type satisfies them.
  **Done, and the signatures are written down in [spsc.rs](crates/windows-waitable-queues/src/spsc.rs)'s module documentation before the type**, as
  the item asked. `push`/`pop` take **`&self`**, not `&mut self`: the latter would also make
  single-producer sound and is what several SPSC crates use, but it cannot generalize to a shape where
  several threads push through a shared handle, and one spelling has to serve every shape.
  Cardinality is carried by the auto traits instead -- the handles are `Send` but **not `Sync`** and not
  `Clone`, so "single" is a fact the compiler checks. A multi-producer shape relaxes exactly one cell of
  that table.
  **Sabotage-verified rather than merely green.** Six deliberate defects, each confirmed to fail the
  suite: a drop loop starting at zero instead of `head`, an off-by-one in the full test, `Full` reported
  where `Disconnected` is owed, `pop` not advancing `head`, `push` not advancing `tail`, and a mask of
  `capacity` instead of `capacity - 1`.
  **One sabotage was NOT caught, and it is recorded rather than smoothed over:** weakening the producer's
  `Acquire` load to `Relaxed` leaves the suite green. That is a genuine limit of stress testing, not a
  missing test -- an ordering bug needs an interleaving the hardware and scheduler must be coaxed into
  producing, and neither ARM64 nor x86-64 will oblige on demand. Queued as M31.6.
  **A harness defect worth remembering:** the first sabotage sweep reported "not caught" for the
  `pop`-does-not-advance case, because the detector matched on the string `test result: FAILED` and the
  test process had instead died with `STATUS_HEAP_CORRUPTION`, which prints no such line. Nine tests had
  in fact failed. A sabotage harness that recognizes only one failure shape will eventually certify a
  hole that is not there -- or miss one that is. Detect by exit code.
  The same defect then bit the *gate*: piping `cargo clippy` through `Select-String` makes
  `$LASTEXITCODE` report the filter's status, not cargo's, so a clean-looking `exit=0` was hiding a real
  `-D warnings` failure (`clippy::doc_overindented_list_items`, seven sites in `windows-platform-probes`,
  actual exit 101) that CI would have caught. Fixed in the preceding commit. Redirect with `*>` and read
  `$LASTEXITCODE` before any pipe.

- [x] **M30.4** -- The doorbell, as its own reviewable unit: a queue-owned **manual-reset** event created
  **lazily**, so a polling-only consumer allocates no kernel object. Level semantics -- signalled exactly
  when the consumer has something to observe. **The reset must be atomic with the observation that there
  is nothing to take; the signal need not be** (C-1b measured why: a late signal is a spurious wakeup, a
  stale reset is a lost one). Hand it out as a borrowed handle plus an owned duplicate, per the
  file-watcher's precedent.
  **Landed together with M30.5 in one commit, because these two items are not independent and the
  checklist was wrong to split them.** A `Doorbell` that no queue calls is dead code, and this workspace
  builds with `-D warnings`, so M30.4 cannot compile on its own. Recorded as an acknowledged
  structuring defect rather than worked around by widening the type's visibility to silence the lint --
  making an API public to dodge a warning is a real design decision taken for a fake reason.
  Delivered as [src/doorbell.rs](crates/windows-waitable-queues/src/doorbell.rs): lazily created (a poll-only consumer allocates no kernel object,
  asserted, not assumed), manual-reset, with `handle` / `owned` / `signal` / `clear`. The redundant
  signal is skipped through an `AtomicBool` mirroring the event, which is sound in exactly one
  direction -- see the done-note on M30.5 for the asymmetry that permits it.

- [x] **M30.5** -- Join the two, and **sabotage-verify the lost-wakeup guard**: a test that reverses the
  reset and the emptiness check must deadlock, and must stop deadlocking when the order is restored. A
  wakeup invariant asserted only by a passing test is a test of nothing -- this is the same discipline
  the ioring crate's `wait_then_drain` and the M17.4 calibration established.
  **Done. The guard is `Consumer::arm`, which clears the doorbell and *then* checks emptiness** -- the
  reverse of the order that reads naturally, which is why it needed proving rather than asserting. The
  sabotage test drives the race deterministically on one thread (an interleaving that must be hit to
  prove a point is not one to leave to the scheduler) and ends in a real bounded `WaitForSingleObject`:
  reversed, it returns `WAIT_TIMEOUT` with an item sitting in the queue -- the lost wakeup, reproduced;
  correct, the check finds the item and never waits at all.
  **Corrected after review: the deterministic test named above tested a *copy* of the wrong order,
  not the real `arm`.** `arm_reversed_racing` is a hand-written duplicate with the two statements
  swapped, so it could only ever show that *a* reversed order is wrong -- it could not detect the real
  `arm` being reversed. Measured: sabotaging the real `arm` was caught in **one run out of three**,
  because detection then depended on two threads interleaving inside a window tens of nanoseconds wide.
  This is the anti-pattern CONTRACT INTEGRITY rule 1 names -- a second copy of a rule checks the copy,
  not the rule -- and it was found only because the sweep was re-run under a tighter bound and the
  result flipped. `Consumer::arm` now carries a `#[cfg(test)]` hook that fires between the clear and the
  check, so a test drives the **real** `arm` through that exact window on one thread. Caught every run
  since.
  **Thirteen sabotages, twelve defects and one control, all behaving as expected.** Caught: push not
  signalling; producer `Drop` not signalling; `arm` checking before clearing; `arm` not creating the
  doorbell before checking; the final drain returning nothing; `clear` resetting the event but not the
  mirror flag; auto-reset instead of manual-reset; the event created already signalled. Three of those
  are caught **as hangs rather than failures**, which is the correct shape for a lost-wakeup defect.
  **The control matters as much as the defects:** removing the skip-redundant-signal optimisation must
  *not* fail, and does not -- so the suite is asserting the contract rather than the implementation.
  **The sweep paid for itself twice, and neither finding came from reading the code.**
  (1) A test gap: the drain-after-disconnect guard sat in a race window no test could reach, so
  breaking it changed nothing. Fixed by extracting it as `Consumer::finish`, a named step a test can
  call directly instead of hoping to schedule the window.
  (2) A harness defect: one sabotage inserted `if false { signal(); }` beside the live call instead of
  deleting it, so it sabotaged nothing and the resulting pass read as a hole in the tests. A sabotage
  that does not sabotage is worse than none, because it retires a question that was never asked --
  always confirm the injected defect actually changes behaviour before believing a "not caught".

## Moved 2026-09-09 21:35:00 -04:00 -- ship-topology M16: PR #56 tenth review round, the SH-3.1.1 diff review

From [CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md), which remains open
at M2-M6, M14, M15 and M-inf.

> **Six of these items are superseded.** SH-16.5, SH-16.8, SH-16.9, SH-16.11, SH-16.12 and SH-16.13
> are all the same piece of work seen from different angles -- reshaping the machine memory topology
> -- and they now live in
> [crates/windows-topology-sys/COMPLETED-CHECKLIST.md](crates/windows-topology-sys/COMPLETED-CHECKLIST.md) as a plan of
> their own, numbered `MMT-*`. They are left here, unchecked and marked, rather than deleted: each
> records how the defect was *found*, which the new plan does not repeat.
>
> **Read the new plan for what to do; read these for why.** The six that remain live here are the
> review round's own findings, already fixed.

**This round is the one [SH-3.1.1](CHECKLIST-ship-topology-and-queues.md#m3-land-the-branch) asked for**, and it is the first that read the
branch as a *diff* rather than reacting to a reviewer's comment. Five reviewers took non-overlapping
crate scopes across all 200 changed files; seven findings came back, listed here worst-first rather
than by crate.

**Two of them are the reason the round was worth running.** SH-16.1 is a regression this branch
introduced *two commits ago* -- it would have failed the next `windows-ioring-sys` publish, twenty
minutes in, with an error naming the wrong cause. SH-16.2 is a soundness hole in the crate that is
about to freeze its API, in a shape whose selling point is that its ordering arguments are written
down and checked.

**Neither was reachable from a memory of having written the code**, which is exactly what SH-3.1.1
predicted about a 222-commit branch.

- [x] **SH-16.1** -- **`publish-crate.yml`'s sibling-dependency wait cannot handle a `*` requirement,
  so `windows-ioring-sys` can no longer publish.** The wait step derives a concrete version with
  `sed -E 's/^\^//'`, which handles only a caret. Commit `f1fc4eb` on this branch made ioring's
  topology dev-dependency path-only, so `cargo metadata` now reports `req=*` -- **verified, not
  assumed**. `windows-topology-sys` is in `workspace_crates`, so the loop is entered, `dep_version`
  becomes the literal `*`, and `select(.vers == "*")` can never match: 60 attempts x 20 s, then an
  error telling the operator to re-run once the dependency is available, which will never help.
  A versionless path dev-dependency is **stripped from the published manifest entirely**, so there is
  nothing to wait for and the right answer is to skip it.
  Note that `tools/check-publishable.ps1`, added in this same branch to catch "release-managed but
  unpublishable", does **not** catch this -- ioring passes all three of its checks.
  **Done:** `*` is skipped with the reason stated, and any requirement that does not reduce to a
  comparable version (`~1.2`, `>=1, <2`, `=1.2.3`) now fails **immediately** naming the requirement,
  rather than reaching the same twenty-minute timeout by a different route. Verified by extracting the
  `run:` block and exercising it under `bash` against real `cargo metadata` output: ioring's seven
  dependencies now resolve to two waits, four skips and one versionless skip, and the step exits 0.
  A side benefit worth recording, found by getting the harness wrong first: the new check also
  catches a returning CR corruption -- the failure the `tr -d '\r'` above was added for -- because
  `0.1.3\r` is no longer a comparable version. That failure used to be a silent timeout too.

- [x] **SH-16.2** -- **`reserving_mpsc::Reservation::send` wrote a slot with no happens-before edge to
  the consumer's read of the previous occupant.** A slot is freed only by `Consumer::pop`'s
  `head.store(Release)`, and the matching acquire lives in `has_room_beyond_reservations`.
  `Producer::push` gets its edge from that room check; `send` deliberately has none -- the code says
  so -- and its claim CAS is `Relaxed` on every path, so there was no release sequence to inherit
  either. The claim proves the slot is *logically* free, which is not the same as a synchronization
  edge, and the `SAFETY` comment cited "the room check that permitted the claim" on the one path
  where no room check exists.
  **The default configuration was the unsound one**: `Options::tracking_high_water()` accidentally
  repaired it, because the metric's `head.load(Acquire)` sat just before the write. Fixed by making
  that load unconditional, which is where it belonged.

- [x] **SH-16.3** -- **`CancelIo` does not wait, so a test frees an `OVERLAPPED` and an I/O buffer the
  kernel may still write to.** In
  [reopen_by_id_cannot_be_watched.rs](crates/windows-file-watcher/tests/reopen_by_id_cannot_be_watched.rs),
  an overlapped `ReadDirectoryChangesW` is issued into a **stack-local** `OVERLAPPED` and a heap
  buffer, then `CancelIo` is called and both are dropped immediately. `CancelIo` only *requests*
  cancellation; the IRP still completes asynchronously and writes `Internal`/`InternalHigh` into a
  frame that has been reclaimed. Two safety comments assert the opposite of what the code guarantees.
  Aggravated by the helper being called twice back-to-back, so the second call's `overlapped` likely
  lands on the same stack address the first IRP will write into.
  **This crate has already been bitten by this exact class of corruption** -- the
  `STATUS_STACK_BUFFER_OVERRUN` history recorded on the now-removed `reopen_via_existing_handle`.
  Fix by calling `GetOverlappedResult(..., bWait = TRUE)` and accepting `ERROR_OPERATION_ABORTED`
  before either buffer leaves scope.
  **Done, and the wait is measured to be load-bearing rather than assumed.** A probe on the control
  path returned `completed=0, err=995` -- `ERROR_OPERATION_ABORTED` -- proving an IRP really was
  outstanding at the moment `CancelIo` returned and completed only during the wait. Without it, that
  completion landed on a reclaimed frame. The same wait is what makes `Owned`'s later `CloseHandle`
  safe, since closing a handle with I/O outstanding is another cancellation request and not a wait.

- [x] **SH-16.4** -- **`cache_partitions_at_level` counted a domain covering no processors as a
  partition.** An empty `ProcessorSet` is not *equal* to any non-empty one, so deduplication kept it,
  and `is_disjoint` is vacuously true on it, so the pairwise check passed it. A level with one real
  cache plus one empty domain therefore reported two partitions and was treated as dividing a machine
  it does not divide. `Domain` is publicly constructible and `ProcessorSet` has `empty()`, so this is
  reachable by hand and by deserialization -- precisely the input the method promises not to trust.
  Fixed by dropping empty domains, with the contrast against `memory_domains` (which deliberately
  keeps a processor-less domain, D-5) recorded at the filter.

- [x] **SH-16.5** -- **DISCHARGED 2026-09-03 by M5+.4 -- `cache_domain` is `Observed<u32>`, the refusal is gone, and `Slice::same_cache_domain` answers `None` rather than `same` for an unobserved participant.** SUPERSEDED by [crates/windows-topology-sys/COMPLETED-CHECKLIST.md](crates/windows-topology-sys/COMPLETED-CHECKLIST.md) (MMT-*); kept for how it was found.** **`windows-placement-probe` refuses a partially-covering cache level that
  `windows-topology-sys` deliberately hands back.** `outermost_partitioning_cache` documents that
  "full coverage of the online processors is deliberately *not* required"; `places_from_topology`
  treats any online processor the chosen level does not name as `MissingPlacement::CacheDomain` and
  fails the **entire run** with `InvalidData`. Two crates state opposite rules about the same return
  value -- a [CONTRACT INTEGRITY](.github/copilot-instructions.md) defect, not merely a bug.
  Decide the rule **once**, in the crate that owns the topology, and have the consumer ask rather than
  restate. Note the asymmetry that makes the NUMA arm different and correct: for NUMA, `None` has no
  honest value, whereas `cache_domain` is already `Option<u32>`.
  **BLOCKED on
  [DESIGN-SESSION-2026-09-02-cache-locality-model.md](design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md)
  -- and *not* for want of a consumer.** The fix was implemented; implementing it surfaced a design
  question the fix would have silently answered. The primitive it adds is a single "which cache domain
  is this processor in", which **is** the single-boundary collapse that session is about, so landing it
  would prejudge the outcome. The prototype compiled, and its topology-side tests passed and were
  sabotage-verified; it was reverted deliberately and preserved outside the repository as
  `sh-16.5-prototype.patch`.
  **Unblocked 2026-09-03, and superseded rather than resumed.** The session's questions were answered
  as `D-13` through `D-21`, and the answer is *not* the primitive this item prototyped: under
  [D-19](crates/windows-topology-sys/DESIGN-NOTES.md#d-19) the unified relation set with its
  inclusion order replaces a single per-processor cache-domain lookup, so the prototype would have
  landed the collapse the session existed to remove. The contradiction is fixed by `MMT` **M2+.5** and
  **M5+.4** instead. The patch is kept as the record of what was tried and why it was not taken.

- [x] **SH-16.8** -- **DISCHARGED 2026-09-03 by M2 -- the granularity order carries all seven kinds and any depth, and `minimal_shared` is the meet rather than a single cache level.** SUPERSEDED by [crates/windows-topology-sys/COMPLETED-CHECKLIST.md](crates/windows-topology-sys/COMPLETED-CHECKLIST.md) (MMT-*); kept for how it was found.** **The locality model collapses a seven-kind, any-depth topology onto one cache
  boundary, and nothing records that as a choice.** Raised by the engineer during the SH-16.5 fix, and
  confirmed: `windows-topology-sys` hardcodes no level count (`level` is a `u8`, and a regression test
  already guards against a consumer sweeping `1..=4`) and models `Group`, `Package`, `Die`, `Module`,
  `Core`, `Cache` and `Memory` -- but `outermost_partitioning_cache` selects one level and discards the
  rest, `ProcessorPlace::cache_domain` is one scalar, and `Placement` carries three tiers.
  Three consequences, all verified: "same cache" denotes **a different boundary on different machines**,
  so a label is not portable across records; `CrossCache` conflates "different L2, same L3" with
  "different L3" on any machine with two live boundaries; and it has already cost a row in this
  project's own matrix -- the x64 host's "cannot express `same cache, same class`" note in
  [DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md) is attributed to hardware, but
  those sixteen processors do share one L3, so a per-level model would express it.
  Gated on the session above, which carries the design space and the open questions.
  **Scope addition from [D-13](crates/windows-topology-sys/DESIGN-NOTES.md):** the audit that decision
  performed over every `Option` in the crate found exactly one site that documentation cannot fix.
  `DomainKind::Memory::memory_bytes` is unambiguous from `discover`, which always sets `None`, but a
  **description's** `None` conflates "the description omitted the field" with "this node's capacity is
  genuinely unknown" -- the two are the same value today. Whatever representation this item lands must
  cover it, since absence becoming first-class is precisely the fix.
  **Direction now settled** by the engineer: presence and observation must be modeled, not
  collapsed into an `Option`. "Win32 did not report it" and "it was found not to be present" are
  different facts, and the representation must be built for **observed connectivity** rather than
  for a ladder of levels with optional rungs. That rules out the SH-16.5 prototype's `Unknown` arm,
  which merges both. Shape still open.

- [x] **SH-16.9** -- **DISCHARGED 2026-09-03 by M5+.3 -- the rule has one implementation, which `windows-platform-probes` now asks rather than restates.** SUPERSEDED by [crates/windows-topology-sys/COMPLETED-CHECKLIST.md](crates/windows-topology-sys/COMPLETED-CHECKLIST.md) (MMT-*); kept for how it was found.** **The "outermost partitioning cache" rule is stated three times, and two of the
  three disagree.** `MachineMemoryTopology::outermost_partitioning_cache` requires more than one partition **and**
  pairwise disjointness. `Observation::outermost_partitioning_cache` in `windows-platform-probes` is
  `caches.iter().filter(|c| c.domains > 1).max_by_key(|c| c.level)` -- **no disjointness check** --
  computed over a `CacheLevel` summary that crate builds itself, even though it already depends on
  `windows-topology-sys`. On a hand-built or deserialized topology with overlapping domains the two
  crates give different answers to the same question. `windows-placement-probe` restates it a third
  time by rebuilding the map from the partition list, which is SH-16.5.
  A [CONTRACT INTEGRITY](.github/copilot-instructions.md) defect of the exact shape the rules name:
  a rule re-encoded by a consumer rather than derived from the owner. Note the ordering -- fixing
  this by pointing both consumers at today's method would have to be redone once SH-16.8 lands, so
  either fix it now and accept the rework, or sequence it after the design session.

- [x] **SH-16.10** -- **`GetSystemCpuSetInformation` is not consumed anywhere, so a whole Win32
  topology model is unexposed.** The crate consumes all seven `GetLogicalProcessorInformationEx`
  relations, but `SYSTEM_CPU_SET_INFORMATION` is a *second, parallel* model carrying at least
  `LastLevelCacheIndex` -- Windows's own LLC grouping, which is a **different answer** from
  "outermost partitioning cache" and would be directly comparable against it -- plus
  `SchedulingClass`, `AllocationTag`, `EfficiencyClass`, and per-processor `Parked` / `Allocated` /
  `RealTime` state.
  Raised by the engineer's question of whether we expose everything a real system would reveal
  through the Win32 API set. Today the answer is **no**.
  Note `Parked` and `Allocated` bear directly on **thread counts and assignments**, one of the three
  decisions the model exists to serve, so this is a gap already costing a named use rather than
  speculative completeness.
  **Ungated, and split, because the gating premise was wrong.** This said "gated on SH-16.8, since
  what shape it lands in depends on the model". That conflated two things: *acquiring* the data and
  *reconciling* it with what `GetLogicalProcessorInformationEx` already reports. Acquisition does
  not depend on the model at all -- CPU Sets is a **cheap OS read**, in the same class as the walk
  this crate already does, and nothing about reading it presumes a granularity representation. Only
  reconciliation depends on the model, and that is now SH-16.13.
  Field list **verified against `windows-sys 0.61.2`** rather than recalled: `Id`, `Group`,
  `LogicalProcessorIndex`, `CoreIndex`, `LastLevelCacheIndex`, `NumaNodeIndex`, `EfficiencyClass`,
  a union carrying `AllFlags` (`Parked` / `Allocated` / `AllocatedToTargetProcess` / `RealTime`), a
  union carrying `SchedulingClass`, and `AllocationTag`. All five APIs are present
  (`GetSystemCpuSetInformation`, `GetThreadSelectedCpuSets`, `SetThreadSelectedCpuSets`,
  `SetThreadSelectedCpuSetMasks`, `SetProcessDefaultCpuSets`) and `Win32_System_SystemInformation`
  is already an enabled feature, so there is no manifest change and no blocker.
  **Done.** `src/cpu_set.rs` walks the records with the same buffer discipline the relationship walk
  uses -- size first, advance by each record's own `Size`, read every field unaligned -- and
  `MachineMemoryTopology::discover` now populates `MachineMemoryTopology::cpu_sets`. Carried as
  `Option<Vec<CpuSet>>` where `None` means **not observed**, which a hand-built or deserialized
  topology genuinely is; that is the honest use of `Option`, one absence rather than two collapsed
  together. `#[serde(default)]` so descriptions written before the field still load.
  **Nothing is reconciled**, per duplicate-then-decide. SH-16.13 owns that.
  **The live dump justified the caution.** On the x64 host, CPU Sets reports **one** distinct
  `LastLevelCacheIndex` across all sixteen processors, while `outermost_partitioning_cache` reports
  **eight** partitions at L2. Both are right -- Windows names the *last* level, the derivation names
  the outermost level that *divides* -- so a merge treating `LastLevelCacheIndex` as "the cache
  domain" would have collapsed eight shard groups into one on this machine. Kept as a test asserting
  the *relationship* (Windows's grouping is never finer) rather than the host's numbers.
  It also confirms the matrix-hole argument from
  [DESIGN-SESSION-2026-09-02-cache-locality-model.md](design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md):
  this is the host recorded as unable to express `same cache, same class`, and a second source now
  says all sixteen share an LLC, so that row is real rather than inferred.
  **One thing is verified only against the SDK's documented bitfield order, not against Windows:**
  the four flag bit positions. Every processor on this host reads `parked=false, allocated=false,
  allocated_to_target_process=false, real_time=false`, which is consistent with a process that has
  requested no CPU-set allocation but confirms no bit position. `each_flag_is_read_from_its_own_bit`
  checks the decode is self-consistent, not that it matches the OS. Confirm against a parked
  processor or an explicit `SetProcessDefaultCpuSets` before relying on the flags.

- [x] **SH-16.13** -- **DISCHARGED 2026-09-03 by M3+.1.2 -- CPU Sets are folded into the relation set and carried beside the walk, so both observers are visible per relation.** SUPERSEDED by [crates/windows-topology-sys/COMPLETED-CHECKLIST.md](crates/windows-topology-sys/COMPLETED-CHECKLIST.md) (MMT-*); kept for how it was found.** **Reconcile the CPU-set observation with the relationship walk.** `CoreIndex`,
  `NumaNodeIndex` and `EfficiencyClass` **duplicate** facts `GetLogicalProcessorInformationEx`
  already reports, from a different kernel path -- so this is not redundancy to remove, it is a
  **second independent observer of the same relations**, and the two can disagree under a hypervisor
  or where one path is stale.
  This is the concrete instance of the design session's "can one relation hold several
  observations?" question, which until now rested on the file-handle spike's agree/disagree
  reasoning about a different subject. It is no longer speculative: two Win32 sources describe the
  same processor's NUMA node and efficiency class today.
  Per [PLATFORM INTEGRITY](.github/copilot-instructions.md)'s duplicate-then-decide rule, SH-16.10
  lands the CPU-set data as its **own** observation alongside the existing domains, without merging.
  This item is the merge-or-delete decision, made when the model settles rather than pre-empted.
  Gated on SH-16.8.
  Note it also bears on SH-16.12: CPU Sets carries `EfficiencyClass` as a plain `u8` with **no
  sentinel**, so it is a cleaner source for the field whose `capacity` encoding collides with
  "unknown".

- [x] **SH-16.11** -- **DISCHARGED 2026-09-03 by M5+.5 -- `distances` and the `Distances` type are deleted.** SUPERSEDED by [crates/windows-topology-sys/COMPLETED-CHECKLIST.md](crates/windows-topology-sys/COMPLETED-CHECKLIST.md) (MMT-*); kept for how it was found.**
  **And now ANSWERED, in the opposite direction to what this item proposed.**
  [D-20](crates/windows-topology-sys/DESIGN-NOTES.md#d-20) rules that the crate does not go below the
  Win32 topology APIs, so a fact Win32 does not report is not one the crate has: `distances` is
  **deleted, not filled**. The removal is `M5+.5` in
  [crates/windows-topology-sys/COMPLETED-CHECKLIST.md](crates/windows-topology-sys/COMPLETED-CHECKLIST.md). Everything
  below is the reasoning that led there and is kept for that; it no longer describes work. **`MachineMemoryTopology::distances` is a field for a fact Win32 cannot supply, it is never
  populated, and the measurement that would fill it already exists elsewhere.** `discover()`
  hardcodes `distances: None`, every other construction sets `None`, and no consumer reads the
  field. Windows exposes no API for NUMA node distance -- ACPI carries SLIT, Win32 does not surface
  it -- so measurement is the only source. `windows-placement-probe` **already measures the
  equivalent** through `node_pairs_measured()`, producing per-node-pair handoff cost with ring
  placement, and renders it as a table that goes nowhere else.
  This is the canonical case for the whole model: under the bar that the model must be usable
  **without further measurement**, a consumer shaping memory allocation must today either run the
  probe at decision time -- forbidden -- or guess. Gated on SH-16.8, and on the open question of
  which component owns the measurement phase.
  **Corrected while stating [EP-D-3](crates/topology-planner/DESIGN-NOTES.md#ep-d-3): the
  wording above reads as an oversight, and it is not one.** The field is documented as being for a
  fed-in description, because Windows exposes no user-mode SLIT reader -- accurate, and deliberate.
  Two sharper problems replace the one this item claimed.
  **First, `distances` can never carry `Measured` provenance, by construction.** Its only inputs are
  hand construction (defaulting to `Synthetic`) and deserialization (capped at `Restored` by
  `downgraded_to`), and `discover()` hardcodes `None`. So populating it would not help: a planner on
  a real machine still could not obtain trustworthy distance *for that machine*.
  **Second, even populated it answers the wrong question.** The matrix is SLIT-shaped -- one
  symmetric, workload-independent scalar per pair -- while the residency decision is directional,
  since the producer writes and the consumer reads. `D-9` in
  [crates/windows-topology-sys/DESIGN-NOTES.md](crates/windows-topology-sys/DESIGN-NOTES.md) already
  anticipated exactly this and deferred it, naming an attributed edge list that "would absorb HMAT,
  **asymmetry**, and multi-hop CXL fabrics", with the trigger being that "scalar distance
  demonstrably mismodels a machine somebody is tuning for". `D-8` keeps the JSON schema outside
  semver specifically to make that revision cheap.
  **The trigger is approached but not met, and the difference is a measurement nobody here can
  take.** The probe treats direction as real -- four numbers per undirected edge, and its code says
  "a hop is not symmetric even though the link is" -- but no run has *shown* those numbers differ,
  because both development hosts report a single NUMA node and every such run prints "VACUOUS ON
  THIS MACHINE". Take that measurement on multi-node hardware before reopening D-9 on asymmetry
  grounds, not after.

- [x] **SH-16.12** -- **DISCHARGED 2026-09-03 by M5+.1, subsumed by M4+.2 -- the shard-set surface has no sentinel, so `0` is never overloaded.** SUPERSEDED by [crates/windows-topology-sys/COMPLETED-CHECKLIST.md](crates/windows-topology-sys/COMPLETED-CHECKLIST.md) (MMT-*); kept for how it was found.** **`Processor::capacity` uses `0` as both a legitimate efficiency class and a
  sentinel for "not known", and the two collide on the common case.** It is computed
  `online.then(|| find the owning Core domain).flatten().unwrap_or(0)`, so `0` means the processor is
  offline, *or* is online but named by no `Core` domain, *or* genuinely has efficiency class zero.
  The third is **every processor on every non-hybrid machine**, so the sentinel is not a rare
  collision -- it is the usual value.
  Found by [crates/topology-planner](crates/topology-planner/DESIGN-NOTES.md#ep-d-1)
  EP-1.1 while checking what a shard planner can rely on, and it is worse for that consumer than for
  most: Windows orders efficiency class with `0` as **least** performant, so on a hybrid part an
  unknown processor is indistinguishable from an efficiency core. A policy excluding efficiency cores
  would silently drop a processor that may be a performance core; a policy tiering them would put it
  in the wrong tier. Neither shows up in a functional test.
  **A third instance of the pattern SH-16.8 exists to fix**, and the one not previously swept -- the
  others being `ProcessorPlace::cache_domain`'s `Option<u32>` (SH-16.5) and
  `MachineDescription::cpu_model`, where the same conflation was noticed and solved with a side
  boolean. Note this one is *worse* than an `Option`: a sentinel that collides with a valid value
  cannot be distinguished even by a careful caller. Gated on SH-16.8, since the fix is the same
  question -- how absence is represented -- and doing it twice would be doing it twice.
  Note `DomainKind::Core { efficiency_class }` already carries the value without a sentinel, so the
  interim guidance is to read that instead; the defect is that `capacity` exists and looks usable.

- [x] **SH-16.6** -- **The thread-stack NUMA spike's `deep_probe` measures the shallow end of its own
  filler, so the discrimination it exists to make is inert.** The stack grows down, so `filler[0]` is
  the deepest address and `filler[last]` sits immediately below the caller's frame -- but the probe
  takes `&raw const filler[last]`, landing very likely on the same page as the shallow probe rather
  than 64 KiB away. The spike would then report "not first touch" on a machine where placement *is*
  by first touch: a confident wrong answer, in a file whose whole point is avoiding those. Both ends
  are already touched, so probing `filler[0]` is a one-token change.
  **Done, and the defect was worse than reported.** Printing the three addresses on all three spike
  threads showed the old probe was not merely *likely* on the shallow probe's page -- it was on the
  **same page every time**, 209 bytes away, where the review had estimated "at worst adjacent". So
  `shallow.node != deep.node` compared one page against itself and could not fire even in principle.
  After the change the two probes are 16 pages apart on every thread. Measured on all three threads
  (`0x...dff7c0` vs `0x...dff6ef` -> same page; vs `0x...def6f0` -> 16 pages), then the instrumentation
  was removed.

- [x] **SH-16.7** -- **A `windows-thread-ambient-sys` test claims a restore-failure it never
  injects.** `release_reports_a_genuine_restore_failure_and_restores_on_drop_even_without_it` asserts
  the *opposite*: it `expect`s the release to succeed and both closing assertions check that restore
  worked. Its siblings in `declared/tests.rs` and `error_mode/tests.rs` do force genuine failures; this
  one inherited the name without the failure-injection half.
  **Done, by renaming -- and the review's stated hazard did not hold.** It reported that
  `TransactionGuard::release`'s error path "reads as covered when it is not". Checked rather than
  taken: the path **is** covered, by `explicit_release_reports_an_injected_restore_failure` in the
  same file, via a `FaultPoint::TransactionSet` injection built for it. Verified by running both.
  So the defect was only ever the name. Renamed to
  `release_and_drop_each_restore_a_real_entry_transaction`, and the comment now records *why* the
  sibling naming does not apply -- a transaction restore either sets a real handle or clears to
  "none", and both succeed, so unlike a null WOW64 cookie or `SEM_NOALIGNMENTFAULTEXCEPT` there is no
  naturally-rejecting value to provoke. Written down so the missing half is not re-attempted.
