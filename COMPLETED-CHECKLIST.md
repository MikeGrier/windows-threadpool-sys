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
