# Checklist: workspace

Workspace-level and cross-crate work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M8 — Fifth review round on PR #3

Two findings, both verified. Both are in code this branch introduced, and one of them is in the fix from the
previous round -- worth stating plainly rather than filing as routine.

- [x] **FR-1** — Close the wrap window in the generation sequence.

	**Gap:** [`M7`](COMPLETED-CHECKLIST.md) replaced a wrapping `fetch_add` with `fetch_add` plus a `store` that
	pins the counter at its exhausted value. Those are two operations, and the counter is *already wrapped to
	zero* between them. A second thread arriving in that window takes 0 -- then 1, 2, ... -- and mints
	successfully, which is precisely the recycled-generation aliasing the guard was added to prevent. The fix
	narrowed the window rather than closing it, while its documentation claimed the counter was pinned.

	**Target:** a single atomic update that refuses to increment past `u64::MAX`, so the counter never
	transiently holds a wrapped value and there is no window to arrive in. Test it under contention at the
	boundary, not just single-threaded, since single-threaded tests are exactly what missed this.

- [ ] **FR-2** — Require exclusive access for the safe blocking adapters.

	**Gap:** `BlockingEndpoint` holds only an `OwnedHandle`, so it is automatically `Send + Sync`. Its five safe
	adapters -- `read`, `write`, `read_scatter`, `write_gather` and `ioctl` -- all take `&self` and call the
	`unsafe` `run`, whose contract states that no other operation may be outstanding on the endpoint. Nothing
	enforces that: two threads sharing the endpoint can each have an `OVERLAPPED` in flight, and `run` waits on
	the *handle*, which is signalled by either completion. A call can therefore return another operation's
	result and hand back buffers the kernel is still writing into. Safe code can reach this, so it is a
	soundness hole rather than a misuse.

	**Target:** take `&mut self` in the safe adapters, so exclusivity is a borrow-check error rather than a
	documented rule -- the same protection cleanup-group members already get, and zero-cost. `run` stays
	`unsafe` with its precondition, which is the right home for that obligation; a caller who genuinely wants to
	share an endpoint wraps it in a `Mutex` explicitly. Record the reasoning, since "the blocking backend is
	single-operation" is now enforced by the type rather than only stated in the module documentation.
