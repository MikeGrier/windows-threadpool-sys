# Checklist: workspace

Workspace-level and cross-crate work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M4 — Review findings on PR #3

Six unresolved review threads on
[PR #3](https://github.com/MikeGrier/windows-threadpool-sys/pull/3), all verified against the code before
being accepted. Several are undefined-behaviour paths reachable from safe code, and two of them undermine
guarantees this very branch introduced -- the identity work fixed one instance of an aliasing hazard while
leaving other routes to the same hazard open.

- [x] **PR-1** — Make cancellation validate and act under one lock, and route every backend through it.

	**Gap:** `cancel` calls `OperationRegistry::is_live`, which releases the mutex before the caller invokes
	`CancelIoEx`. A completion can reclaim the operation in that window and a concurrent submission can reuse
	the address, so the native cancel reaches the wrong operation -- the exact failure generations were added to
	prevent. Separately, `AssociatedSocket::cancel` never consults the registry at all.

	**Target:** a registry operation that checks liveness and performs the native cancellation while still
	holding the guard, used by the IOCP, socket, and `TP_IO` cancellation paths alike.

- [ ] **PR-2** — Compare full operation identities in the typed claim tokens.

	**Gap:** `FileIo`, `ScatterGatherIo`, `SocketIo`, and `DeviceIoControlIo` match a completion by comparing
	`completion.overlapped_ptr()` against a stored address only. A token that outlives an unclaimed completion
	can match a *later* completion that reused the address and then call `Completion::claim::<P>` with the wrong
	payload type. That is type confusion, and it is reachable without `unsafe` on the caller's side.

	**Target:** every typed token compares `completion.id()` against the full identity it was issued.

- [ ] **PR-3** — Make `CallbackEnviron` actually retain the pool borrow it appears to take.

	**Gap:** `set_pool` accepts `&ThreadpoolPool` but stores only the raw `PTP_POOL`; the environment has no
	lifetime and no owned field. Safe code can set a pool, drop it, and then create an object from the
	still-live environment with a dangling pool value. M4-1 closed the "safe function takes a raw isize" hole
	but left this one, which reaches the same undefined behaviour by a longer route.

	**Target:** the environment carries the pool's lifetime, so it cannot outlive the pool it names.

- [ ] **PR-4** — Contain panics in the work trampoline.

	**Gap:** the `TP_WORK` trampoline invokes the user callback without `catch_unwind`, unlike every other
	trampoline in the crate. A panicking work callback unwinds into a non-unwind FFI boundary and aborts the
	process, while the crate documentation promises panics are contained.

- [ ] **PR-5** — Defer token-requested timer re-arming until the callback has returned.

	**Gap:** `TimerFiring::rearm_after` arms immediately, so its delay runs from the moment of the call rather
	than from the end of the firing. A callback that re-arms early and then runs longer than the delay can be
	entered again concurrently, which contradicts `ThreadpoolTimer`'s stated "never overlaps" property and makes
	an `Fn + Sync` callback unexpectedly reentrant.

	**Target:** the trampoline applies a token-requested arming after the callback returns, making the
	documented "measured from the end of each firing" behaviour true unconditionally. Arming from *outside*
	while a callback runs remains possible, so the type's documentation must state the guarantee it actually
	provides rather than a stronger one.
