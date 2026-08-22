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

## Moved 2026-08-22 -- M11: caller-supplied owned buffers, so no adapter forces a copy

## M11: caller-supplied owned buffers, so no adapter forces a copy

The adapters already transfer buffer ownership to the operation and hand it back on completion -- the
"protracted borrow" that completion-based I/O requires, since the kernel touches the memory after the
submitting call returns. What they get wrong is hardcoding **which** owned buffer: `Vec<u8>`. A caller
holding a `Box<[u8]>`, an `Arc<[u8]>`, a `bytes::Bytes`, an alignment-constrained buffer, or one from a
pool must convert, and every one of those conversions is the data copy this crate exists to avoid.

The target: **no adapter copies a caller's bytes by default**, and a caller can hand over whatever owned
buffer it already has, including a shared one. A naive caller passing a slice still pays a copy, which is
fine and expected -- but it has to be *visible at the call site*, never something the adapter does behind
a performance-minded caller's back.

- [x] **M11.1** -- Add `IoBuf` (readable) and `IoBufMut` (writable) to a new `buf` module, re-exported
  from the crate root. Both are `unsafe` traits, because the whole contract is a promise the compiler
  cannot check: the address must be **stable** for the value's life, so a type whose accessor returns a
  fresh address each call (or reallocates) is what makes the operation write into freed memory. Require
  `Send + 'static`, matching what the leaked operation storage already needs. Provide impls for `Vec<u8>`,
  `Box<[u8]>`, and `PageBuffers` (read and write), and for `Arc<[u8]>` and `&'static [u8]` (read only --
  neither can hand out `&mut`, which is exactly why the split exists rather than one trait). Unit tests
  including a stability check that the pointer does not move across a move of the value.

  Read buffers are required to be **fully initialized** for `bytes_len()` bytes rather than tracking an
  initialized prefix through `MaybeUninit`. A caller-supplied pooled buffer is initialized once and reused
  for the life of the pool, so the cost is per-pool, not per-operation, and that buys an API with no
  `set_init`-style obligation to get wrong. Record the trade in DESIGN-NOTES (M11.6).

- [x] **M11.2** -- Make the file adapters generic over the buffer: `AssociatedEndpoint::read<B: IoBufMut>`
  takes the buffer to read into instead of a length it allocates, `write<B: IoBuf>` takes any readable
  owned buffer, and `FileIo<B>` / the `Started` payload carry `B` so `claim` returns the caller's own
  buffer back. Allocating a `Vec` becomes the caller's visible `vec![0; n]` rather than something the
  adapter does for them.

- [x] **M11.3** -- Make the socket adapters generic the same way: `recv<B: IoBufMut>`, `send<B: IoBuf>`,
  `SocketPayload<B>`, `SocketIo<B>`. The `WSABUF` is built from the buffer's stable pointer and length
  rather than from a `Vec`'s.

- [x] **M11.4** -- Make `device::ioctl` generic over both of its buffers (`I: IoBuf` for input, `O:
  IoBufMut` for output), replacing the `output_len` parameter that currently makes the adapter allocate.
  `DeviceIoControlIo<O>` returns the caller's output buffer. The blocking form follows.

- [x] **M11.5** -- Sweep for remaining forced copies now that the traits exist, and fix or record each:
  the blocking adapters (which take slices legitimately, since they block for the whole operation), the
  scatter/gather path (already owns `PageBuffers`; confirm no conversion sneaks in), and any `to_vec` /
  `clone` left in the adapters or their tests.

- [x] **M11.6** -- Record in [DESIGN-NOTES.md](DESIGN-NOTES.md): why completion-based I/O forces owned
  buffers rather than slices (the token has no `Drop`, and even one could be defeated by `mem::forget`, so
  no borrow can be made to span the operation); why the blocking adapters may still take slices; why the
  trait is `unsafe` and what the stable-address contract means; why read buffers are fully initialized
  instead of init-tracked; and why the split into `IoBuf`/`IoBufMut` exists (so a shared `Arc<[u8]>` can be
  written from but never read into). Update the README's adapter section.

## Moved 2026-08-22 -- M12: the two gaps M10 and M11 left open

## M12: close the two gaps M10 and M11 left open

Both of these were carried as prose notes at the foot of this file after M11. That was the wrong form --
CHECKLIST files are action-only, and a parked item belongs in an `M-inf` bucket with a real ID, not a
paragraph. On review neither belongs in `M-inf` either: one had no blocker at all, and the other's blocker
turned out to be a design fork the engineer has now settled.

- [x] **M12.1** -- Implement `IoBuf` for `&'static mut [u8]`, the natural handoff for a leaked or
  statically-allocated pool. It is sound on every count the trait asks for: exclusive by construction,
  stable because the referent is `'static` and never moves, and already `Send + 'static`. Implement
  `IoBufMut` for it too -- unlike `Arc<[u8]>` and `&'static [u8]`, a `&'static mut` is *exclusive*, so it
  is a legitimate read destination and excluding it would be the arbitrary half of the split.

  M11 left this out on the stated grounds that "nothing has asked for it," which is precisely the
  reasoning the PRIME DIRECTIVE forbids. Recorded here so the correction is visible rather than silent.

- [x] **M12.2** -- Add `AssociatedSocket::set_notification_modes`, gated behind a capability probe, and
  update `classify_socket` in the same change so the two can never disagree.

  The three questions M11 parked, and their answers:
  - *Where does it live?* On `AssociatedSocket`, taking `&mut self`. Sockets have no unassociated stage to
    hang provenance on, and adding an `UnassociatedSocket` purely for symmetry would be churn for its own
    sake. Setting after association is safe because the flag only takes effect at I/O time; `recv`/`send`
    keep taking `&self`, so a caller sets the mode once and then submits freely.
  - *Probe or trust?* **Probe.** Win32 restricts socket skip-on-success to Layered Service Providers that
    return IFS handles, and a socket wrongly put in that mode reports `Issued::Pending` for an operation
    whose packet was suppressed -- the exact rundown wedge M10.5 fixed for handles, rediscovered on the
    socket side. Trusting the caller would re-open a bug we have already paid for once. The probe reads
    this socket's own `WSAPROTOCOL_INFOW` via `getsockopt(SOL_SOCKET, SO_PROTOCOL_INFOW)` and requires
    `XP1_IFS_HANDLES` in `dwServiceFlags1`, refusing with `io::ErrorKind::Unsupported` otherwise. That is
    narrower and more accurate than the `WSAEnumProtocols` sweep the flag's own documentation suggests: it
    asks about the provider that actually created *this* socket rather than about every LSP installed on
    the machine.
  - *Feature layout?* The `socket` feature gains `Win32_Storage_FileSystem`, because
    `SetFileCompletionNotificationModes` lives there and the socket family now genuinely needs it. This is
    consistent with the DESIGN-NOTES rule -- a family turns on what that family needs -- rather than an
    exception to it; record the widening there.

  `classify_socket` becomes mode-aware exactly as `fs`/`device` did in M10.5, reading the count from
  `WSARecv`/`WSASend`'s `lpNumberOfBytesTransferred` out-parameter (currently passed as null) via the
  operation's `sync_bytes` cell.

- [x] **M12.3** -- Cover both: that a `&'static mut [u8]` round-trips through a write and a read with its
  address intact; that the probe accepts an ordinary TCP socket (the base Winsock provider is IFS) and that
  the setter's refusal path returns `Unsupported` rather than a Win32 error; and that a socket in
  skip-on-success mode reports `Started::Completed` with no packet queued and nothing left outstanding,
  paired against a default socket that is always `Pending` -- the same shape as
  `tests/skip_on_success_adapters.rs` uses for files and devices.

- [x] **M12.4** -- Record in [DESIGN-NOTES.md](DESIGN-NOTES.md): why the socket setter probes rather than
  trusting, and what it probes; why it sits on the associated socket where the handle side sits on the
  unassociated endpoint; the `socket` feature's widening; and that `&'static mut [u8]` is the one shared-
  looking type that *is* a legal read destination, because it is exclusive.
