# Checklist: workspace

Workspace-level and cross-crate work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M3 — Operation identity must not alias a recycled operation

An `OperationId` is currently just the address of an operation's storage. Reclaiming an operation returns that
address to the allocator, so a later operation can be handed the same address, and an `OperationId` retained
from the earlier operation then names the later one. Because `CompletionPort`-backed `AssociatedEndpoint::cancel`
and `ThreadpoolIo::cancel` are **safe** functions that act purely on that address, a stale identity can silently
cancel an unrelated live operation.

This was reproduced directly: cancelling and draining an operation, then submitting a fresh one, recycled the
identity within 64 cycles, at which point the retained identity named the new live operation. The triggering
pattern is the primary use case for cancellation -- a timeout firing while a completion is in flight -- so the
race is not contrived. The defect is shared by both backends because it lives in the shared submission seam.

The fix stamps every submission with a process-wide monotonic generation and has each backend keep a registry of
live identities, so cancellation can reject an identity that no longer names the operation the caller meant.

- [x] **AB-1** — Give every submitted operation a process-wide monotonic generation, carry it in `OperationId`,
	and have both backends keep a live-identity registry that cancellation validates against.

	**Scope:** `windows-overlapped-io-sys` gains the global generation counter and the widened `OperationId`
	(`new`, `generation`, plus `Hash` now that the value is a stable key). The IOCP backend replaces its
	`outstanding: AtomicUsize` with the registry and validates in `AssociatedEndpoint::cancel`;
	`windows-threadpool-sys` replaces `ThreadpoolIo`'s `Mutex<usize>` with the registry and validates in
	`ThreadpoolIo::cancel`. `Completion::id` and `IoCompletion::id` return full identities. A stale or unknown
	identity is rejected with `ErrorKind::NotFound` **without** calling `CancelIoEx`, so a recycled address is
	never handed to the kernel on the caller's behalf.

	**This item is deliberately large and lands as one commit.** Widening `OperationId` breaks every construction
	and consumption site in both crates at once; splitting it would leave the workspace uncompilable between
	commits. Records the decision in both crates' DESIGN-NOTES, including the reversal of the IOCP backend's
	previously documented lock-free-submission property and the alternatives rejected.

- [ ] **AB-2** — Test in `windows-overlapped-io-sys` that a retained identity cannot cancel a recycled
	operation on the IOCP backend, including a direct reproduction of address recycling.

- [ ] **AB-3** — Test in `windows-threadpool-sys` that a retained identity cannot cancel a recycled operation
	on the `TP_IO` backend, and that live identities still cancel normally.
