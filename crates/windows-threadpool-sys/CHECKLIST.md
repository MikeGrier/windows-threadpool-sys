# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md). Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

Milestones M1 (callback environment), M2 (work submission and callback ownership), and M3 (the `TP_IO` backend
over the shared seam) are complete.

## M4 — Safe abstractions and documentation

M4 was originally three items, the first of which was "implement safe work, timer, wait, and I/O abstractions".
Execution showed that item was mis-sized and partly already satisfied, so it is restructured here:

- The **work** abstraction landed in M2 (`ThreadpoolWork`) and the **I/O** abstraction in M3 (`ThreadpoolIo`),
	so only **timer** and **wait** remain to be built.
- Timer and wait are independent deliverables with their own SDK contracts (arming, rearming, disarming,
	and callback drain), so each gets its own implementation and test item rather than sharing one.
- Building on the callback environment surfaced a soundness defect that must be fixed before any further
	object type consumes it, which is now the first item.

- [x] **M4-1** — Close the `CallbackEnviron` soundness hole: add an owned `ThreadpoolPool` and change
	`set_pool` to accept it, and make `set_cleanup_group` `unsafe` pending a full cleanup-group design.

	**Gap:** `set_pool` and `set_cleanup_group` are **safe** functions that accept a raw `PTP_POOL` /
	`PTP_CLEANUP_GROUP` (both bare `isize`). Any non-zero value the caller invents is later dereferenced by the
	thread pool, so safe code can cause undefined behavior. `set_library` is already `unsafe` for exactly this
	reason, so the two are also inconsistent with their own neighbour.

	**Target:** an owned `ThreadpoolPool` (`CreateThreadpool` / `CloseThreadpool`, plus thread minimum and
	maximum) that `set_pool` borrows, so validity is carried by the type. `set_cleanup_group` becomes `unsafe`
	with a documented contract rather than gaining a safe wrapper here, because a sound cleanup group requires
	changing how *every* callback object is closed (see M4-6) and that cannot land in this item.

- [x] **M4-2** — Implement a safe `ThreadpoolTimer` over `CreateThreadpoolTimer`, `SetThreadpoolTimer`,
	`IsThreadpoolTimerSet`, `WaitForThreadpoolTimerCallbacks`, and `CloseThreadpoolTimer`, covering one-shot
	relative, periodic, and absolute due times, plus disarming. `Drop` must disarm before draining callbacks so
	a periodic timer cannot requeue during teardown.

- [x] **M4-3** — Test the timer: one-shot firing, periodic repetition, absolute due time, disarming before and
	after firing, `is_set` transitions, cancellation of queued callbacks, and destruction while a callback is
	executing.

- [ ] **M4-4** — Implement a safe `ThreadpoolWait` over `CreateThreadpoolWait`, `SetThreadpoolWait`,
	`WaitForThreadpoolWaitCallbacks`, and `CloseThreadpoolWait`. The object owns its waitable handle, so the
	handle cannot be closed while a wait is pending, and the callback receives a token that can rearm the wait
	for the next activation, since the SDK requires explicit rearming per activation.

- [ ] **M4-5** — Test the wait: signalled activation, timeout activation, explicit rearming across several
	activations, disarming, and destruction with a wait armed.

- [ ] **M4-6** — Design and implement safe cleanup-group membership across every callback object.

	**Blocker this resolves:** `CloseThreadpoolCleanupGroupMembers` releases its member objects, which must not
	then be closed individually. Every object type currently closes itself unconditionally in `Drop`, so a
	group-owned object would be double-closed. Making this sound requires each of `ThreadpoolWork`,
	`ThreadpoolTimer`, `ThreadpoolWait`, and `ThreadpoolIo` to know whether it is group-owned and skip its own
	close, which is why it follows their implementations rather than preceding them.

- [ ] **M4-7** — Add API examples and generated documentation covering the whole surface: crate-level guidance,
	runnable doc examples for work, timer, wait, and I/O, and a README that reflects the finished API.
