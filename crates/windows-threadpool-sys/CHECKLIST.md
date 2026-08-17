# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md).

## M1 — Callback environment

- [ ] Implement SDK-equivalent `TP_CALLBACK_ENVIRON_V3` initialization and mutation — the header-only helpers
	that `windows-sys` does not emit. See [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

- [ ] Unit-test callback-environment init and mutation against the documented default field values (version,
	priority, size).

## M2 — Work submission and callback ownership

- [ ] Implement and test one end-to-end work submission abstraction over `CreateThreadpoolWork`,
	`SubmitThreadpoolWork`, `WaitForThreadpoolWorkCallbacks`, and `CloseThreadpoolWork`.

- [ ] Validate the callback ownership model with the work abstraction before extending it to timers and waits.

## M3 — TP_IO backend over the shared seam

> **CROSS-COMPONENT PREREQUISITE:** the submission seam in `windows-overlapped-io-sys`
> (`into_overlapped` / `from_overlapped` / `reclaim_overlapped`) must be available first. See
> [../windows-overlapped-io-sys/COMPLETED-CHECKLIST.md](../windows-overlapped-io-sys/COMPLETED-CHECKLIST.md).

- [ ] Wire the `windows-threadpool-sys` → `windows-overlapped-io-sys` dependency (path plus version), and update
	the publish workflow so the overlapped crate releases before this one.

- [ ] Implement the `TP_IO` backend over the shared seam with balanced `StartThreadpoolIo` /
	`CancelThreadpoolIo` accounting and callback-driven reclamation.

- [ ] Test `TP_IO` across the behavioral matrix: immediate failure, immediate success, pending completion,
	cancellation, and object rundown with operations outstanding.

## M4 — Safe abstractions and documentation

- [ ] Implement safe work, timer, wait, and I/O abstractions.

- [ ] Test callback completion, cancellation, and destruction on Windows.

- [ ] Add API examples and generated documentation.
