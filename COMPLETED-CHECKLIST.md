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
