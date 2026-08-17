# Completed checklist: windows-threadpool-sys

Append-only archive of completed work items. See [CHECKLIST.md](CHECKLIST.md) for pending work and
[PLANS.md](PLANS.md) for plan status. Design decisions are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md).

## Moved 2026-08-17 — M1 callback environment, M2 work submission, M3 TP_IO backend

### M1 — Callback environment

- [x] **M1-1** — Implement SDK-equivalent `TP_CALLBACK_ENVIRON_V3` initialization and mutation — the
	header-only helpers that `windows-sys` does not emit. See [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

- [x] **M1-2** — Unit-test callback-environment init and mutation against the documented default field values
	(version, priority, size).

### M2 — Work submission and callback ownership

- [x] **M2-1** — Implement and test one end-to-end work submission abstraction over `CreateThreadpoolWork`,
	`SubmitThreadpoolWork`, `WaitForThreadpoolWorkCallbacks`, and `CloseThreadpoolWork`.

- [x] **M2-2** — Validate the callback ownership model with the work abstraction before extending it to timers
	and waits.

### M3 — TP_IO backend over the shared seam

- [x] **M3-1** — Wire the `windows-threadpool-sys` → `windows-overlapped-io-sys` dependency (path plus
	version), and update the publish workflow so the overlapped crate releases before this one.
	*(completed 2026-08-17 15:37:00 -04:00)*

	The publish workflow blocks the `windows-threadpool-sys` job until the required
	`windows-overlapped-io-sys` version is live on crates.io, because `cargo publish`'s verification build
	resolves the versioned dependency from the registry rather than the workspace path. That makes the
	release order independent of the order the two tags are pushed.

- [x] **M3-2** — Implement the `TP_IO` backend over the shared seam with balanced `StartThreadpoolIo` /
	`CancelThreadpoolIo` accounting and callback-driven reclamation.
	*(completed 2026-08-17 15:56:00 -04:00)*

	Added `ThreadpoolIo` and `IoCompletion`. One counter serves as both the pool's start/cancel accounting
	and the crate's rundown state, since an unbalanced start is exactly an operation whose storage the
	kernel or pool still owns. Required widening the shared seam with `OperationId::from_ptr`, which was
	fixed in `windows-overlapped-io-sys` rather than worked around here.

- [x] **M3-3** — Test `TP_IO` across the behavioral matrix: immediate failure, immediate success, pending
	completion, cancellation, and object rundown with operations outstanding.
	*(completed 2026-08-17 16:06:00 -04:00)*

	21 integration tests in `tests/tp_io_behavioral_matrix.rs` covering the five matrix states plus the
	accounting and reclamation invariants at scale (512 file reads, 256 simultaneously outstanding pipe
	reads, 8-thread concurrent submission) and the edge cases: a panicking callback, reads past EOF,
	zero-length reads, mixed payload types, unclaimed completions, and repeated idempotent rundown.

	The matrix surfaced that an operation identity is unique only among *simultaneously outstanding*
	operations — reclaiming an operation returns its storage address to the allocator, which may reissue it.
	That contract was undocumented and is now recorded on `OperationId` and in the overlapped crate's
	design notes.

## Moved 2026-08-17 — M4 safe abstractions and documentation

M4 was restructured during execution. Its original first item bundled work, timer, wait, and I/O, but work had
landed in M2 and I/O in M3, so only timer and wait remained; those became separate implementation and test
items. Two items were added from defects found while building: M4-1 (a `CallbackEnviron` soundness hole) and
M4-8 (the one-shot/periodic timer split).

- [x] **M4-1** — Close the `CallbackEnviron` soundness hole: add an owned `ThreadpoolPool` and change
	`set_pool` to accept it, and make `set_cleanup_group` `unsafe` pending a full cleanup-group design.
	*(completed 2026-08-17 17:00:00 -04:00)*

	`set_pool` and `set_cleanup_group` were **safe** functions accepting a raw `PTP_POOL` / `PTP_CLEANUP_GROUP`
	(bare `isize`) that the thread pool later dereferences, so safe code could cause undefined behavior.
	`set_library` next door was already `unsafe` for exactly this reason.

- [x] **M4-2** — Implement a safe timer over `CreateThreadpoolTimer`, `SetThreadpoolTimer`,
	`IsThreadpoolTimerSet`, `WaitForThreadpoolTimerCallbacks`, and `CloseThreadpoolTimer`.
	*(completed 2026-08-17 17:05:00 -04:00)*

- [x] **M4-3** — Test the timer across one-shot, periodic, absolute, disarming, cancellation, and destruction.
	*(completed 2026-08-17 17:05:00 -04:00)*

	The tests corrected the implementation's specification: `IsThreadpoolTimerSet` reports whether a due time is
	set, not whether the timer will fire again, so a one-shot timer stays set after firing. The `is_set`
	documentation had said the opposite.

- [x] **M4-4** — Implement a safe `ThreadpoolWait` that owns its waitable handle and rearms per activation.
	*(completed 2026-08-17 17:12:00 -04:00)*

	The object owns the handle so it cannot be closed under a pending wait, and the callback receives a
	`WaitActivation` carrying `rearm`, since the SDK consumes the arming on each activation.

- [x] **M4-5** — Test the wait across signalled and timeout activation, rearming, disarming, and destruction.
	*(completed 2026-08-17 17:12:00 -04:00)*

	Also de-flaked the identity tests in both crates: they required the allocator to naturally reuse an address,
	which is not guaranteed. Natural reuse is now reported rather than asserted, and the hazard is covered
	deterministically by tests that synthesize a stale generation at a live address via `OperationId::from_parts`.

- [x] **M4-8** — Split the timer into `ThreadpoolTimer` (one-shot) and `ThreadpoolPeriodicTimer`, and give each
	callback a token. *(completed 2026-08-17 17:25:00 -04:00)*

	The first implementation followed the platform and modelled both kinds with one object, where a `period`
	argument silently changed the concurrency contract. A periodic timer may queue its next tick while the
	previous one is still running, so its callback must tolerate overlapping with itself; a one-shot never
	overlaps. `TimerFiring::rearm_after` gives non-overlapping repetition measured from the end of each firing;
	`PeriodicTick::stop` lets a periodic timer end itself. A zero period is rejected rather than silently
	degenerating to a one-shot.

- [x] **M4-6** — Design and implement safe cleanup-group membership across every callback object.
	*(completed 2026-08-17 17:33:00 -04:00)*

	Option (A) of the three considered: the group creates and owns its members. Flagging objects as
	"group-owned" is insufficient, because each also owns a heap callback context that is only safe to free once
	the bulk release has finished — a moment an individual object cannot observe. Use-after-release is a compile
	error, pinned by a `compile_fail` doc test. Thread-pool I/O is excluded on purpose: a `TP_IO` object must not
	be closed with an operation outstanding, and a bulk release cannot satisfy that.

- [x] **M4-7** — Add API examples and generated documentation.
	*(completed 2026-08-17 17:20:00 -04:00)*

	Crate-level guidance, runnable doc examples for every object type, a rewritten README, and docs.rs metadata
	pinning a Windows target — without which documentation for this Windows-only crate would fail to build.
