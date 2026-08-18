# Checklist: workspace

Workspace-level and cross-crate work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M9 — Sixth review round on PR #3

Five findings, all verified, all correct. Three are in work this branch introduced in the previous two rounds,
including one that is a direct failure to apply a lesson written down in the last round's own design note.

- [x] **FZ-1** — Document that re-arming a still-signalled wait overlaps its own callback.

	**Gap:** [`WaitActivation::rearm`](crates/windows-threadpool-sys/src/wait.rs) says nothing about
	concurrency. On a manual-reset event the handle stays signalled, so re-arming from inside the callback
	queues the next activation immediately -- before the current one returns. Measured: re-arming at the top of
	a 20ms callback produced **7529 runs with 5110 overlapping entries in 400ms**, against 1 run and no overlap
	for an auto-reset event.

	A caller who assumes the one-shot's non-overlap guarantee carries over will get unbounded concurrent
	callbacks and a runaway that looks like a pool bug. Document the condition, the two ways out (reset the
	event before re-arming, or make the callback tolerate concurrency), and cover it with a test so the
	behaviour is pinned rather than merely described.

- [x] **FZ-2** — Validate file read lengths before allocating.

	**Gap:** [`FR-3`](COMPLETED-CHECKLIST.md) put the length check *after* `vec![0_u8; len]` in both `read`
	adapters, so `read(u32::MAX as usize + 1, ..)` tries to allocate more than 4GiB before returning
	`InvalidInput`. The regression test added with it can therefore abort instead of exercising the error path,
	and the design note claiming "checked before allocating" was true only of the scatter and ioctl paths.

- [x] **FZ-3** — Reject oversized socket lengths.

	**Gap:** `socket.rs` holds a *third* copy of the capping helper, with four call sites across both backends,
	and allocates its receive buffer before capping. This is the same unhonoured-length defect fixed for
	`device` and then for `fs` -- and the last round's design note ends with "when a defect is found in a helper,
	check whether the helper has siblings", which is exactly the check not performed. The advice was right; it
	was written and not followed.

	Until this lands, [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md)
	claims a completeness the crate does not have, so its wording is part of this item.

- [ ] **FZ-4** — Stop the identity tests mutating global state, and remove their false-pass mode.

	**Gap:** two defects in one test. Its four worker threads each `take_hook`/`set_hook`/restore the
	*process-global* panic hook concurrently, so an interleaving can leave the no-op hook installed -- silencing
	diagnostics for every other test in the binary, which Cargo runs in parallel threads of one process. And the
	observer threads can be scheduled after the minters have already finished, in which case they sample nothing
	and the test passes against the broken implementation it exists to catch.

	**Target:** a non-panicking seam so the concurrent test raises no panics and touches no hook at all, leaving
	the panic itself to a single-threaded `should_panic` test; and a barrier so observers are provably running
	before the boundary is crossed. Be explicit about what the resulting test does and does not guarantee.
