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

- [x] **M4-4** — Implement a safe `ThreadpoolWait` over `CreateThreadpoolWait`, `SetThreadpoolWait`,
	`WaitForThreadpoolWaitCallbacks`, and `CloseThreadpoolWait`. The object owns its waitable handle, so the
	handle cannot be closed while a wait is pending, and the callback receives a token that can rearm the wait
	for the next activation, since the SDK requires explicit rearming per activation.

- [x] **M4-5** — Test the wait: signalled activation, timeout activation, explicit rearming across several
	activations, disarming, and destruction with a wait armed.

- [x] **M4-8** — Split the timer into `Timer` (one-shot) and `PeriodicTimer`, and give each callback a token.

	**Why this reopens M4-2/M4-3.** The first implementation followed the platform and modelled both kinds with
	one object, where a `period` argument silently changed the semantics. That hides the property that makes
	periodic timers confusing in practice: the pool may queue the next callback **while the previous one is
	still running**, so a periodic callback must tolerate overlapping with itself. A one-shot timer never has
	that problem. Two behaviours that differ in their concurrency contract should be two types, so the hazard is
	attached to the type that has it rather than to an argument.

	**Target:** `timer::Timer` fires exactly once per arming; `timer::PeriodicTimer` is constructed with its
	period and repeats. Each callback receives a token, because the useful operations are only available from
	inside a firing: `TimerFiring::rearm_after` / `rearm_at` (which is how a caller gets *non-overlapping*
	repetition -- the next delay is measured from when the previous callback finished), and
	`PeriodicTick::stop` (how a periodic timer ends itself). Each type's documentation must state its own
	concurrency contract and point at the other as the alternative.

	The types are named `ThreadpoolTimer` and `ThreadpoolPeriodicTimer`, matching `ThreadpoolWork`,
	`ThreadpoolWait`, `ThreadpoolIo`, and `ThreadpoolPool`. The callback tokens (`TimerFiring`,
	`PeriodicTick`) carry no prefix, matching `WaitActivation`: they are callback parameters rather than
	owned thread-pool objects.

- [x] **M4-6** — Design and implement safe cleanup-group membership across every callback object.

	**Decision taken:** option (A) below -- the group creates and owns its members. A draft implementation
	exists at `.scratch/cleanup_group.draft.rs`; it was parked because M4-8 changes the timer API it builds on,
	and it must gain a periodic-timer member type before it lands.

	**BLOCKED — awaiting a design decision.** Not blocked on effort; blocked on choosing an ownership model.

	**The problem.** `CloseThreadpoolCleanupGroupMembers` releases every member object at once. Afterwards the
	members must not be used or closed individually. Two things break today:

	1. Every object type closes itself unconditionally in `Drop`, so a group-owned object would be
		double-closed.
	2. Each object also frees its heap callback context in `Drop`. For a group-owned object the context is only
		safe to free once the group has finished releasing members (that is what guarantees no callback is
		running), and an individual object cannot know whether that has happened.

	So the group must own both the member lifetimes and the contexts; marking objects "group-owned" is not by
	itself sufficient.

	**Options considered:**

	- **(A) The group creates and owns its members.** `CleanupGroup::create_work(..) -> WorkMember<'_>`, with
		members borrowing the group and the group holding the boxed contexts. `close_members` takes `&mut self`,
		so the borrow checker forbids calling it while a member is alive. Fully sound and compiler-enforced;
		costs a parallel set of constructors and prevents members outliving the group.
	- **(B) Objects carry a shared handle to the group.** Each object learns at creation that it is group-owned,
		skips its own close, and hands its context to the group to free at `close_members`. Keeps one type per
		object kind, but use-after-release stays a runtime concern rather than a compile-time one.
	- **(C) Decline to support cleanup groups.** Keep `set_cleanup_group` `unsafe` permanently and document that
		group members must not be this crate's owned types.

	**Worth weighing before choosing:** per-object `Drop` already performs correct teardown for every type here,
	so a cleanup group adds bulk-teardown convenience rather than a safety property this crate is missing.
	Option (C) is therefore a legitimate outcome and not merely a deferral -- but if it is chosen it must be
	recorded as a deliberate decision in the design notes, not left implicit.

- [x] **M4-7** — Add API examples and generated documentation covering the whole surface: crate-level guidance,
	runnable doc examples for work, timer, wait, and I/O, and a README that reflects the finished API.
