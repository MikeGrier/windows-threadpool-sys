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

## Moved 2026-08-16 — Buffer-owning `DeviceIoControl` adapters (M11)

### M11 — `DeviceIoControl` adapters (`device` feature)

- [x] Implement synchronous `BlockingEndpoint::ioctl(code, input, output_len)` behind the `device`
	feature, issuing an overlapped `DeviceIoControl` and returning `io::Result<(Vec<u8>, usize)>`. The adapter
	owns its buffers, but the generic raw-code seam is `unsafe`: an arbitrary control code may embed pointers to
	storage the adapter cannot own. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Implement submission `AssociatedEndpoint::ioctl(code, input, output_len)` behind `device` returning a
	typed `DeviceIoControlIo` token whose `claim(&Completion)` recovers the output buffer and byte count; the
	generic raw-code seam is `unsafe` for the same reason. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Integration test (`device`): an `FSCTL` query on a real file through both the blocking and IOCP `ioctl`
	adapters, upholding the self-contained safety contract in an `unsafe` block. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

## Moved 2026-08-18 — M16, thirteenth review round on PR #3

### <a id="mf-1"></a>MF-1 — Deregister an operation when its packet is dequeued, so a held completion cannot deadlock the port. *(completed 2026-08-18 02:42:11 -04:00)*

Reported by review and **confirmed by reproduction**: safe code could dequeue a `Completion`, hold it, and drop
the `CompletionPort`. `Drop` runs `run_down`, which blocked in `get(INFINITE)` waiting for a packet that had
already been delivered -- an unconditional hang with no timeout.

Registration now ends at dequeue rather than at reclamation, separating the port's obligation to deliver a
packet from the completion's ownership of the operation's storage. `Completion` no longer holds an
`Arc<PortState>`, since nothing it does needs one. `outstanding()` correspondingly no longer counts a
dequeued-but-held completion. Recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md) under *Registration ends at dequeue, not at reclamation*.

Two regression tests were added and verified against a faithful revert: with the old behaviour restored exactly,
those two fail and the other 61 pass, so they target this defect and nothing else.

### <a id="mf-2"></a>MF-2 — Reattach the `checked_len` documentation to `checked_len`. *(completed 2026-08-18 02:42:11 -04:00)*

`scatter_gather_len` carried an opening paragraph describing the conversion `checked_len` performs, while
`checked_len` itself had no documentation at all -- an edit splice, the same class of damage as the stray `///`
in the previous round. The paragraph was moved back to the function it describes.

## Moved 2026-08-22 -- M10: skip-on-success completion notification modes made usable end to end

## M10: skip-on-success completion notification modes

Reopened because [DESIGN-NOTES.md](DESIGN-NOTES.md) already decided that
`SetFileCompletionNotificationModes` "is modeled as an opt-in endpoint provenance attribute ... behind a
feature gate", and that decision was never transcribed into a work item -- the orphaned-decision failure
mode the repo's "design notes are not a work queue" rule exists to prevent. The core submission seam
already implements skip-on-success correctly on both backends (`Issued::Completed` /
`Submitted::Completed`, with `CancelThreadpoolIo` balancing on the `TP_IO` side), and both have passing
behavioral-matrix coverage for it. What is missing is at the two ends: nothing in the crate can *set* the
mode, and the buffer-owning adapters cannot *express* an already-complete result, so they reject one.

`FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` removes a packet queue/dequeue and a worker wakeup per
synchronously-completing operation. It is a real win for workloads that complete synchronously often
(cached reads, small socket sends, loopback) and a no-op for genuinely asynchronous ones, so it belongs to
the caller to opt into per endpoint, not to this crate to decide.

The adapter change is deliberately **breaking**: every adapter submission returns a two-state outcome
rather than a token. Taken now, while the crate has no adopters, rather than carried as a parallel
surface forever.

**Re-planned after M10.3.** M10.1-M10.3 were written believing the return shape was the only thing
stopping an adapter from reporting a synchronous completion. Writing M10.6's end-to-end test disproved
that: an ioctl on a skip-on-success endpoint *hung*. The adapters' `classify_issued` / `classify_socket`
still answer `Issued::Pending` on immediate native success, which is correct only for an endpoint that is
not in skip mode. On one that is, no packet is ever queued, the operation stays counted as outstanding,
and `CompletionPort::run_down` spins forever waiting for it. Two things must exist before an adapter can
classify correctly, and neither was in the original plan: the adapter has to *know* the endpoint's mode
(M10.5), and it needs somewhere with the operation's lifetime to receive the synchronous byte count
(M10.4), because the native calls currently pass `null` for that out-parameter. M10.4 and M10.5 are
inserted here; the original M10.4/M10.5 became M10.6/M10.7.

- [x] **M10.1** -- Add the `Started<T, P>` outcome enum (`Pending(T)` / `Completed { payload,
  bytes_transferred }`) to a new `started` module, re-exported from the crate root, with the accessors an
  ordinary caller needs (`is_pending`, `is_completed`, `pending`, `completed`) and unit tests. This is the
  shape every adapter's submission returns once M10.3 lands; it is `Submitted` with the `Failed` arm
  folded into `io::Result`'s `Err` and the operation storage already reduced to its payload.

- [x] **M10.2** -- Add the notification-mode setter as an endpoint provenance attribute:
  `NotificationModes { skip_completion_port_on_success, skip_set_event_on_handle }` plus
  `UnassociatedEndpoint::set_notification_modes`, feature-gated on `fs` (it needs
  `Win32_Storage_FileSystem`). Document that the mode is irreversible once set (Win32: "after a mode has
  been set for a file handle, it cannot be removed"), that it takes effect only once the handle is
  associated with a port, and that for sockets it is only compatible with LSPs returning IFS handles.
  Cover with a test that sets the mode and observes a synchronous no-packet completion through the raw
  seam.

- [x] **M10.3** -- Convert every buffer-owning adapter's submission to return `io::Result<Started<..>>`,
  deleting the `finish` / `finish_scatter` / `finish_device` / `finish_socket` error arms that currently
  reject a synchronous completion: `fs::read`, `fs::write`, `fs::read_scatter`, `fs::write_gather`,
  `device::ioctl` (both the `AssociatedEndpoint` and blocking forms as applicable), `socket::recv`,
  `socket::send`. Each `Completed` arm reduces `Operation<P>` to the same payload its token's `claim`
  yields (the `Vec<u8>`, the `PageBuffers`, `DeviceIoPayload::output`, `SocketPayload::buffer`) so the two
  paths report the same shape. Update every in-crate caller and test.

- [x] **M10.4** -- Give `Operation` a synchronous byte-count cell in its header, reachable from the
  `OVERLAPPED` identity the way `payload_ptr_from_overlapped` reaches the payload, and use it as the
  `lpNumberOfBytesTransferred` / `lpBytesReturned` out-parameter every adapter currently passes as `null`.
  It has to live in the pinned operation rather than on the adapter's stack: `DeviceIoControl` documents
  that with a non-null `lpOverlapped` the count "is meaningless until the overlapped operation has
  completed", so the kernel may write it *after* the submitting call returns, and a stack local would be
  a dangling write whenever the operation goes asynchronous. Keep the cell before `payload` so the
  reclaim thunk's offset stays identical for every `P`.

- [x] **M10.5** -- Carry `NotificationModes` on the endpoint through association so the adapters can
  classify: record what `set_notification_modes` established on `UnassociatedEndpoint`, propagate it into
  `AssociatedEndpoint` (and expose it), and make `classify_issued` / `classify_socket` answer
  `Issued::Completed { bytes_transferred }` -- reading M10.4's cell -- exactly when the endpoint is in
  skip-on-success mode *and* the native call returned immediate success, and `Issued::Pending` otherwise.
  This is what "opt-in endpoint provenance attribute" has to mean in practice: the mode is not decoration,
  it is the fact the submission seam needs to answer "will a packet arrive". Document on
  `assume_overlapped` that a mode established behind the endpoint's back (on the raw handle, before it was
  wrapped) must be declared through `set_notification_modes` so the endpoint agrees with the handle;
  the call is additive and idempotent, so re-declaring an already-set mode is safe.

- [x] **M10.6** -- End-to-end coverage: for at least the file and device adapters, set skip-on-success on
  the endpoint, drive an operation that completes synchronously, and assert the adapter returns
  `Started::Completed` with the right bytes and payload, that no packet arrives on the port, and that
  rundown converges immediately (the outstanding count was balanced inline). Pair each with the existing
  non-skip endpoint to assert the same operation returns `Started::Pending` there. Whether a given request
  completes synchronously is the I/O Manager's call, so each test asserts the invariants of whichever arm
  it observes rather than requiring the synchronous one; the non-skip pairing, where `Pending` *is*
  guaranteed, is asserted exactly.

- [x] **M10.7** -- Record the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md): why `Issued` answers "will a
  packet arrive" rather than "did it finish synchronously" (the distinction that makes an immediate `TRUE`
  a `Pending` on a default endpoint and a `Completed` on a skip-mode one), why the adapters return a
  two-state outcome instead of hiding the synchronous case, why the notification mode has to be tracked on
  the endpoint rather than passed per call, and why the synchronous byte count lives in the operation
  header. Update the crate README if it describes adapter return shapes.
