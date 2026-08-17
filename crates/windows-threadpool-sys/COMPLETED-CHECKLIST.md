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
