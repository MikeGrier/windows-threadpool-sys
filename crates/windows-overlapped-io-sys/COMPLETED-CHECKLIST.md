# Completed checklist: windows-overlapped-io-sys

Append-only record of completed checklist groups. Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## Moved 2026-08-16 — Overlapped-I/O foundation (milestones M1–M4)

### M1 — Design and contracts

- [x] Specify the rounded-out overlapped-I/O requirements and the crate boundary.
- [x] Specify the voluntary-rundown-versus-`Drop` contract.

### M2 — Ownership and operation storage

- [x] Implement endpoint ownership and the unsafe provenance seam.
- [x] Implement pinned operation storage and `OVERLAPPED` completion identity.

### M3 — Raw IOCP backend

- [x] Implement the raw IOCP backend: port, association, submission, and cancellation.
- [x] Implement outstanding-operation accounting, generic reclamation, and blocking rundown for the voluntary
	method and `Drop`, with a non-panicking `Drop` diagnostic that names each outstanding operation's submit
	site (the `operation-backtrace` feature adds full backtraces).

### M4 — Blocking backend and submission seam

- [x] Implement and test the blocking `GetOverlappedResult` backend for un-ported endpoints.
- [x] Define the submission seam (`into_overlapped` / `from_overlapped` / `reclaim_overlapped`) consumed by the
	thread-pool `TP_IO` implementation, and dogfood it in the raw IOCP backend.

## Moved 2026-08-16 — Safe endpoint provenance and feature layout (M5)

### M5 — Safe endpoint provenance and feature layout

- [x] Design and implement safe endpoint creators / sealed association to remove reliance on the unsafe
	`assume_overlapped` seam. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Add the gated `windows-sys` feature layout for file, socket, and device operation families, keeping the
	published crate's default feature set minimal. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Integration test: a safe-created endpoint runs a real operation on both the IOCP and blocking backends.

## Moved 2026-08-16 — Behavioral-matrix hardening and shared-port drain semantics (M6)

### M6 — Behavioral-matrix hardening

- [x] Exercise the raw IOCP backend across the behavioral-matrix cases not yet covered: immediate success under
	`FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`, completion identity under many simultaneous operations, and
	results/payloads retained after native endpoint shutdown. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Decide and document multi-endpoint / multi-threaded drain semantics for a shared `CompletionPort` — who
	drains, and how completions for distinct endpoints are attributed during rundown. See
	[DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Integration test: multi-threaded dequeue and rundown on a port shared by several endpoints.

## Moved 2026-08-16 — Safe file operation adapters (M7)

### M7 — Safe file operation adapters (`fs` feature)

- [x] Implement fully-safe synchronous `BlockingEndpoint::read` / `write` behind the `fs` feature, owning the
	buffer and returning `io::Result` with no `unsafe` for the caller. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Implement safe-submission `AssociatedEndpoint::read` / `write` behind `fs` returning a typed `FileIo`
	token whose `claim(&Completion)` safely recovers the buffer and byte count. This item also adds the
	`pub(crate)` payload-pointer-from-`OVERLAPPED` primitive to `operation.rs` (its only consumer), so the two
	land together. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Integration test (`fs`): a safe file write-then-read round-trip on both the blocking and IOCP backends,
	with no `unsafe` in the test's I/O path.

## Moved 2026-08-16 — Safe scatter/gather file adapters (M8)

### M8 — Safe scatter/gather file adapters (`fs` feature)

- [x] Add a page-aligned `PageBuffers` type and fully-safe synchronous `BlockingEndpoint::read_scatter` /
	`write_gather` behind the `fs` feature, owning the buffers and the `FILE_SEGMENT_ELEMENT` array. `PageBuffers`
	lands with this, its first consumer. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Implement safe-submission `AssociatedEndpoint::read_scatter` / `write_gather` behind `fs` returning a typed
	`ScatterGatherIo` token whose `claim(&Completion)` recovers the `PageBuffers` and byte count. See
	[DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Integration test (`fs`): a page-aligned gather-write-then-scatter-read round-trip on both the blocking and
	IOCP backends, with no `unsafe` in the test's I/O path. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

## Moved 2026-08-16 — Safe socket operation adapters (M9)

### M9 — Safe socket operation adapters (`socket` feature)

- [x] Refactor the IOCP submission core into a shared `CompletionPort::submit_with` helper so both handle and
	socket endpoints reuse the outstanding-operation accounting; `AssociatedEndpoint::submit` delegates to it with
	no behavior change. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Implement the IOCP socket backend behind the `socket` feature: `CompletionPort::associate_socket` and an
	`AssociatedSocket` with `recv` / `send` (`WSARecv` / `WSASend`) returning a typed `SocketIo` token whose
	`claim(&Completion)` recovers the buffer and byte count. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Integration test (`socket`): a loopback TCP send-and-receive round-trip through the IOCP socket adapter,
	with no `unsafe` in the test's I/O path. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

## Moved 2026-08-16 — Safe blocking socket backend (M10)

### M10 — Safe blocking socket backend (`socket` feature)

- [x] Implement fully-safe `BlockingSocket::recv` / `send` behind the `socket` feature, issuing `WSARecv` /
	`WSASend` with a per-call `WSACreateEvent` completion event and blocking via `WSAGetOverlappedResult`, with no
	`unsafe` for the caller. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Integration test (`socket`): a loopback TCP round-trip through `BlockingSocket`, with no `unsafe` in the
	test's I/O path. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
