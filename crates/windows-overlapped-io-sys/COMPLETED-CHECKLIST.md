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

## Moved 2026-08-23 — `AssociatedEndpoint` drop-while-outstanding (M1)

Found while designing `windows-ioring-sys`'s M8 (a PR #20 review response): `AssociatedEndpoint` owned its
`handle: OwnedHandle` directly and had no `Drop` impl, so a caller could drop one (closing its handle via
`OwnedHandle`'s own drop) while an operation was still outstanding against it specifically.
`CompletionPort::run_down` was the wrong scope to catch this -- it blocks on the *port's* whole outstanding
count, not any one endpoint's. See [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "Per-endpoint outstanding tracking
and blocking `Drop` (M1)".

### M1 -- `AssociatedEndpoint` drop-while-outstanding

- [x] Track each `AssociatedEndpoint`'s own outstanding count and give `AssociatedEndpoint` a `Drop` that
  blocks until it reaches zero, mirroring `CompletionPort::run_down`'s own drop behavior.
- [x] Integration test: drop an `AssociatedEndpoint` with a real operation still outstanding against it and
  assert the drop blocks until that operation's completion is observed, rather than closing the handle out
  from under a live kernel operation. Added
  `dropping_an_endpoint_drains_its_own_outstanding_operation_before_closing` in
  [tests/overlapped_cancellation.rs](tests/overlapped_cancellation.rs).

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

## Moved 2026-08-22 -- M13: the notification-mode setter belongs to the core, not to the fs family

## M13: the notification-mode setter belongs to the core, not to the `fs` family

- [x] **M13.1** -- Move `Win32_Storage_FileSystem` into the always-on `windows-sys` feature set, ungate
  [`UnassociatedEndpoint::set_notification_modes`](src/endpoint.rs) and the `notification_flags` module, and
  drop the now-redundant `Win32_Storage_FileSystem` entry (and its explanatory comment) from the `socket`
  feature.

  **Gap:** every part of the notification-mode mechanism is core *except the one method that establishes it*.
  `NotificationModes`, `UnassociatedEndpoint::notification_modes`, the mode's carriage through
  `CompletionPort::associate`, and `AssociatedEndpoint::notification_modes` are all ungated; only
  `set_notification_modes` is `#[cfg(feature = "fs")]`. So a consumer enabling only `device` has a mode that
  is permanently `false` and no way to change it, while [`device.rs`](src/device.rs) reads
  `skip_completion_port_on_success` at every submission and `classify_issued` branches on it -- the device
  family's skip-on-success support is unreachable by the exact consumer it was built for.

  **The soundness edge, which is the deciding argument:** `assume_overlapped` is an ungated core `unsafe fn`
  whose documented safety obligation is that a caller who set a notification mode on the raw handle must
  re-declare it, because "a handle silently in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode would have its
  synchronous successes reported as pending, leaving operations outstanding forever." A `device`-only
  consumer is handed that obligation and no way to discharge it. A core contract must not be dischargeable
  only by whoever happened to enable an unrelated operation family.

  **Target:** the gate keyed on an operation family disappears rather than being widened to
  `any(fs, device)`. Widening would fix the reported symptom and leave the next family to rediscover it;
  the capability is a property of an endpoint, and an endpoint does not know its family. Records that this
  supersedes the `default = []` comment's claim that "the core completion machinery needs no operation-family
  `windows-sys` bindings at all" -- true when written, false since `assume_overlapped` took on that
  obligation. `windows-sys` is a bindings-only crate, so the cost is compile-time surface, not emitted code.

- [x] **M13.2** -- Give the device family its own skip-on-success integration test, gated on `device` alone,
  rather than leaving it nested inside a file that requires `fs`.

  **Gap:** [`tests/skip_on_success_adapters.rs`](tests/skip_on_success_adapters.rs) is
  `#![cfg(all(windows, feature = "fs"))]` and carries the device coverage as an inner
  `#[cfg(feature = "device")] mod device`, so the device skip path is only ever exercised when `fs` is *also*
  enabled -- the same coupling as the defect in M13.1, in the tests that were supposed to catch it. Audit
  [`tests/device_ioctl.rs`](tests/device_ioctl.rs) for the same shape.

  **Target:** a `device`-only build sets skip-on-success on an endpoint, issues an ioctl, and observes
  `Started::Completed` with no packet queued and nothing outstanding, paired against a default endpoint that
  is always `Pending` -- the shape M12.3 used for sockets. As there, assert the synchronous arm is actually
  taken by at least one submission, so the test cannot pass vacuously if the mode stops being applied.

- [x] **M13.3** -- Add a CI job that builds, lints, and tests each operation family on its own
  (`--no-default-features --features <family>`) for every family, plus the bare `--no-default-features` core.

  **Gap:** [ci.yml](../../.github/workflows/ci.yml) only ever runs `--all-features` and default (empty)
  features. No individual family combination is built anywhere, which is why M13.1's gap shipped, why M12.2's
  socket-only build had to be checked by hand, and why nothing would stop the next family repeating it.
  `--all-features` is structurally blind to this class of defect: it hides every missing gate.

  **Target:** the check is mechanical and additive -- adding a family to the matrix is a one-line change --
  so the guarantee survives the next family without anyone having to remember it.

- [x] **M13.4** -- Record in [DESIGN-NOTES.md](DESIGN-NOTES.md): that the notification-mode mechanism is core
  rather than per-family, and why (the endpoint owns the capability, the submission seam depends on it, and
  `assume_overlapped`'s contract requires it); and supersede M12.2's note explaining the `socket` feature's
  widening, which M13.1 makes obsolete.

## Moved 2026-10-14 19:25:01 -04:00 -- M15.1: owned versus shared endpoint handles

### <a id="m151"></a>M15.1 -- Investigated owned versus shared endpoint handles; the engineer chose observers. *(completed 2026-10-14 19:25:01 -04:00)*

The findings and the decision are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "Decision -- observers
share the handle; only the endpoint does I/O (M15.1)". Requested by
[win-multi-file-tail-watcher](../win-multi-file-tail-watcher/DESIGN-NOTES.md#tw-d-10).

## Moved 2026-10-14 19:45:28 -04:00 -- M15.2 and M15.4: endpoints share their handle with observers

M15.4 could not land separately: `ThreadpoolIo::new` called the infallible `into_handle` that M15.2 made
fallible, so neither compiled without the other.

### <a id="m152"></a>M15.2 -- Endpoints keep their handle in shared storage and hand out observers. *(completed 2026-10-14 19:45:28 -04:00)*

`UnassociatedEndpoint` stores its handle in a private `SharedHandle`; `AssociatedEndpoint` and
`BlockingEndpoint` hold the new `EndpointHandle` (the storage as it leaves an endpoint, not `Clone`, never
yields an `OwnedHandle`). `EndpointObserver` is `Clone`, `Send` and `Sync`, keeps the handle open, and
implements neither `AsHandle` nor `AsRawHandle`; every endpoint type has `observer()`. `into_handle` now
returns `Result<OwnedHandle, Self>`, refusing while an observer lives. Breaking change. Recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> "observers share the handle; only the endpoint does I/O".

### <a id="m154"></a>M15.4 -- `ThreadpoolIo` keeps the endpoint's shared storage; landed with M15.2. *(completed 2026-10-14 19:45:28 -04:00)*

`windows-threadpool-sys`' `ThreadpoolIo` holds an `EndpointHandle` taken with `into_endpoint_handle`, so an
observer taken before binding outlives it; its `Drop` comments now say the guarantee rests on only this object
submitting through the handle, and that dropping releases its hold rather than closing the handle.

## Moved 2026-10-14 20:07:01 -04:00 -- M15.3: the observer's queries

### <a id="m153"></a>M15.3 -- The observer reads the file's length, and offers an `unsafe` borrow for other queries. *(completed 2026-10-14 20:07:01 -04:00)*

`EndpointObserver::file_len` (behind `fs`) reads `EndOfFile` with
`GetFileInformationByHandleEx(FileStandardInfo)`; tests cover lengths set and appended through another
handle, past 4 GiB on a sparse file, and with write-only access. Sabotage confirmed them: returning
`AllocationSize`, or truncating to 32 bits, each fails. Its error arm is reached by no test -- files, the
NUL device and an unconnected named-pipe server all answer -- and says so at the definition.
`EndpointObserver::handle` is the `unsafe` borrow; its `# Safety` section is the contract.

## Moved 2026-10-14 20:29:52 -04:00 -- M15.5: tests for observers across every backend

### <a id="m155"></a>M15.5 -- Tests: compile-fail doctests, observers across every backend, and cancellation and rundown with observers alive. *(completed 2026-10-14 20:29:52 -04:00)*

Compile-fail doctests on `EndpointObserver` (no `as_handle`, no `as_raw_handle`, not accepted by
`CompletionPort::associate` or `BlockingEndpoint::new`) and on windows-threadpool-sys'
`ThreadpoolIo::observer` (not accepted by `ThreadpoolIo::new`), each with a compiling positive control;
giving the observer both traits turned the two trait doctests red. `ThreadpoolIo` lacked `observer()`,
which M15.4 had missed, so it was added. Runtime tests: every backend's observer reads the length after
its backend drops; on `AssociatedEndpoint` and `ThreadpoolIo`, cancelling, running down and dropping with
an operation pending and an observer alive all drain, and the handle stays open until the last observer
drops. Removing `AssociatedEndpoint::drop`'s explicit cancel made the drop test hang. `BlockingEndpoint`
has no cancellation or rundown; what testing it surfaced is M15.6.

## Moved 2026-10-14 20:46:21 -04:00 -- M15.6, completing M15: owned versus shared handles on endpoints

Every M15 item is now archived in this file under its own heading, and the milestone has left
[CHECKLIST.md](CHECKLIST.md). Requested by
[win-multi-file-tail-watcher](../win-multi-file-tail-watcher/DESIGN-NOTES.md#tw-d-10); its TW-1.2
prerequisite is met.

### <a id="m156"></a>M15.6 -- `BlockingEndpoint` waits on an event of its own, so an observer's query cannot strand it. *(completed 2026-10-14 20:46:21 -04:00)*

The engineer chose option 1 of three. `BlockingEndpoint` creates one manual-reset event and lends it to
each operation's `OVERLAPPED.hEvent` for one `run`, taking it back on every exit; `skip_set_event_on_handle`
is no longer refused. Recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "`BlockingEndpoint` waits on an
event of its own (M15.6)". Tests pin the measured reset, complete a genuinely pending read with the
handle's event suppressed and with observer queries running, and check the event is lent only for the
call. Sabotage: leaving `hEvent` null made the suppressed-event read hang; dropping the take-back failed
the lend test.

## Moved 2026-10-14 21:19:36 -04:00 -- M16.1: the observer's standard information

### <a id="m161"></a>M16.1 -- `standard_info()` reads the whole `FileStandardInfo`; `file_len()` is its shortcut. *(completed 2026-10-14 21:19:36 -04:00)*

`EndpointObserver::standard_info` returns a `StandardInfo` (length, allocation size, link count, delete
pending, directory). The queries live in a new `src/endpoint/query.rs`, child of the endpoint module so they
reach the observer's private handle; `file_len` now calls `standard_info`, so the query exists once. The
kernel writes the two flags as `BOOLEAN` bytes, so the call fills a private `#[repr(C)]` mirror with `u8`
fields, tied to `FILE_STANDARD_INFO`'s layout by compile-time assertions, rather than reading them as Rust
`bool`. Tests cover an ordinary file, lengths across sizes, a sparse file's smaller allocation, hard links
made and removed, a delete requested through another handle (pending until the observer lets go, then
completed), a directory, and every observer agreeing. Sabotage: swapping the two flags failed the delete and
directory tests; swapping length and allocation failed three others.

## Moved 2026-10-14 21:27:57 -04:00 -- M16.2: the observer's file identity

### <a id="m162"></a>M16.2 -- `file_id()` reads the volume serial and 128-bit file ID. *(completed 2026-10-14 21:27:57 -04:00)*

`EndpointObserver::file_id` returns a `FileId` (volume serial number, and the 128-bit ID as the 16 bytes the
file system reports, not reinterpreted as an integer). The `GetFileInformationByHandleEx` call is now one
private `unsafe fn query_info<T>` that every query uses, `standard_info` included. Tests cover observers of
one handle and a second open agreeing, files in one directory differing on one volume, a rename while held
and a hard link both keeping the ID, a file recreated under the same name getting a new one, and a
directory. Measured: the NUL device and a named pipe both refuse `FileIdInfo` with
`ERROR_INVALID_PARAMETER`, so the shared error path is now tested, on the NUL device. Sabotage: returning
success on failure failed that test; zeroing the ID failed the three that compare IDs.

## Moved 2026-10-14 21:33:14 -04:00 -- M16.3: the observer's handle type

### <a id="m163"></a>M16.3 -- `file_type()` classifies any handle, in the core rather than behind `fs`. *(completed 2026-10-14 21:33:14 -04:00)*

The engineer placed it in the core: what counts as a file system is broad. `EndpointObserver::file_type`
returns a `FileType` (`Disk`, `Character`, `Pipe`, `Remote`, `Unknown`, or `Other(u32)` for an undocumented
value) from `GetFileType`, clearing the last error first so a stale one cannot pass for the call's. It lives
in its own always-compiled `src/endpoint/file_type.rs`, since `query.rs` is `fs`-gated as a whole. The mapping
from return value and last error is a pure function, so every branch is tested, the error and undocumented
values included; real handles cover a disk file, a directory, the NUL device, both ends of a named pipe, and
an observer outliving its endpoint. Sabotage: swapping disk and character, and dropping the error branch,
failed five tests. Testing the crate alone also surfaced unused imports left by M15.6, fixed separately in
`a054f8b8`.

## Moved 2026-10-14 21:43:21 -04:00 -- M16.4: the observer's sector geometry and buffer alignment

### <a id="m164"></a>M16.4 -- `storage_info()` and `alignment_info()` read the sector geometry and buffer alignment. *(completed 2026-10-14 21:43:21 -04:00)*

`StorageInfo` carries every `FILE_STORAGE_INFO` field: the four sector sizes, the flags kept as reported
(with `aligned_device()` and `partition_aligned_on_device()` reading the two documented bits), and the two
offsets as `Option`, `None` for `STORAGE_INFO_OFFSET_UNKNOWN`. `AlignmentInfo` keeps the reported mask,
with `is_aligned(address)` (meaningful for any mask) and `byte_alignment()` (`None` unless the mask is one
less than a power of two). `windows-sys` exports the `STORAGE_INFO_*` values only behind
`Win32_System_WindowsProgramming`, so they are named in a local const module. Measured on this machine: the
temp volume reports logical 512 and physical 4096, flags `0xB` (bit `0x8` is named by neither), and an
alignment mask of 1 -- word alignment, far below a page; the NUL device refuses the storage query
(`ERROR_INVALID_FUNCTION`) and answers the alignment query with mask 0. The conversions are pure functions,
tested across every flag, the unknown sentinel on each offset, and masks of and not of the documented form.
Sabotage: inverting the unknown check, swapping the flag bits, and dropping the power-of-two check each
failed its own test.

## Moved 2026-10-14 22:10:23 -04:00 -- M16.5: unbuffered I/O asks for its alignment

### <a id="m165"></a>M16.5 -- The page size is asked of the OS, and `PageBuffers` aligns to what the device reports. *(completed 2026-10-14 22:10:23 -04:00)*

`page_size()` (asked of `GetSystemInfo` once and kept) replaces the public `PAGE_SIZE` constant -- breaking.
`PageBuffers::with_alignment(pages, AlignmentInfo)` aligns the allocation's base to the larger of the page
size and the least power of two clearing every bit of the reported mask; `new` page-aligns as before, and
`alignment()` reports which. Later pages follow at page intervals, so the buffer stays one contiguous
allocation. The open question -- ask once per endpoint or per operation -- dissolved in the building: the
caller asks once, through an observer, when it allocates, and the adapters ask nothing, following M11's "no
adapter allocates on the caller's behalf". The sector size is not enforced; a non-multiple fails the native
call. `fs` now enables `Win32_System_SystemInformation`. The seam for alignments above a page is
`AlignmentInfo` itself, whose mask is a public field: tests build one asking for 16 KiB and round-trip
through blocking scatter/gather, and another round-trip uses what the real endpoint reports. Sabotage:
ignoring the alignment failed two tests; dropping the page floor failed four. Replacing `page_size()` with a
constant 4096 would pass on this machine, whose page is 4 KiB -- the test compares against `GetSystemInfo`
and catches it only where the two differ.

## Moved 2026-10-14 22:18:54 -04:00 -- M16.6: the remote protocol query

### <a id="m166"></a>M16.6 -- `remote_protocol_info()` reads the remote protocol, version, flags and SMB2 capabilities. *(completed 2026-10-14 22:18:54 -04:00)*

`RemoteProtocolInfo` carries the structure version, protocol, major/minor version, revision and flags as
reported, with `loopback()`, `offline()` and `persistent_handle()` reading the three documented bits; `smb2`
is `Some(Smb2Info)` only when the protocol is `WNNC_NET_SMB` (`0x20000`), since the union member is defined
only for SMB. Measured on this machine: a local file refuses the query with `ERROR_INVALID_PARAMETER` (it
does not report an empty protocol), and that is tested. Through `\\localhost\<drive>$` it answers structure
version 4, SMB 3.1.1, flags `0x39` (loopback plus `0x8`, `0x10` and `0x20`, which are named nowhere and kept
as reported), server capabilities `0x6F`, share capabilities `0x80`. The share test is `#[ignore]` because it
needs the administrative share -- administrator rights and the Server service -- and passes here with
`--include-ignored`. The same probe found the kernel refusing a buffer aligned to 4 but not 8 with
`ERROR_NOACCESS`, so every query's buffer is now a private `QueryBuffer<T>` aligned to 8, pinned by a const
assertion; a second ignored share test drives the raw call through a 4-mod-8 buffer and expects the
refusal. Sabotage: dropping the alignment to 1 was not caught by any test (the stack buffer happened to land
8-aligned), which is why the const assertion exists -- dropping it to 4 then failed the build; the SMB check
weakened to "any protocol" and the loopback bit swapped for offline each failed their tests.

## Moved 2026-10-14 22:31:47 -04:00 -- M16.7: the basic-information query

### <a id="m167"></a>M16.7 -- `basic_info()` reads the four times and the attributes, measured current through the handle. *(completed 2026-10-14 22:31:47 -04:00)*

Measured first, on Windows build 26300, on NTFS (the temp directory on `C:`) and ReFS (`Q:`), with the same
results on both. An observer held the file open while a second handle wrote to it; the directory entry was
read by enumerating the directory (`FileFullDirectoryRestartInfo`). Through the observer's handle the
last-write, change and last-access times advanced at the first query after each write. The directory entry
kept the times from before the first write, and the old length, for over four seconds, until the writer
closed, then matched the handle. A later read by path advanced the last-access time through the handle on
NTFS but not in its directory entry; on ReFS it advanced neither. Immediately after a create, the entry's
change time was seen trailing the handle's by under a millisecond (6 of 15 parallel runs), so the kept test
compares the entry with its own earlier state rather than with the handle. The NUL device refuses the query
with `ERROR_INVALID_PARAMETER`, tested.

`BasicInfo` holds the times as `FileTime { ticks }` -- 100-ns intervals since 1601, exactly as reported --
with `to_system_time()`, which is `None` only for a negative count; attributes are the raw
`FILE_ATTRIBUTE_*` word. The conversion is tested against literal dates, not its own constants. The
measurement is kept as a test (`a_write_elsewhere_shows_here_at_once_and_in_the_directory_entry_at_close`),
stable over 30 parallel runs. The sabotages are now recorded in the crate's first
[sabotage.json](sabotage.json), with M16.6's: field swaps, dropped attributes, a wrong epoch and tick unit
are caught; two controls survive, one of them the measured finding that refusing a negative count is an
equivalent mutant on Windows (a cast count overflows `SystemTime` anyway), now said at the site.

## Moved 2026-10-14 22:43:09 -04:00 -- M16.8: the final-path query

### <a id="m168"></a>M16.8 -- `final_path()` and `final_path_as()` read the current path as `Wtf16String`, in every volume and name form. *(completed 2026-10-14 22:43:09 -04:00)*

`final_path_as(VolumeName, PathForm)` exposes every combination `GetFinalPathNameByHandleW` offers --
`Dos`, `Guid`, `Nt` and `Omitted` volumes, `Normalized` and `Opened` names -- and `final_path()` is the
drive-letter, normalized default. The result is `Wtf16String`, filled in place through wtf-string's
`with_capacity` / `as_mut_ptr` / `set_len_from_ffi`; wtf-string is an optional dependency enabled by `fs`,
and is not re-exported, following the repository's other crates. The fill loop (`fill_wide`) is a private
function over a closure, tested against a stand-in for fits, exact fits, one-short, long, growing between
calls, a callee reporting the size it was given, empty initial buffers, failures before and after a retry,
and unpaired surrogates and NULs kept verbatim.

Measured on the temp volume (NTFS): each volume form puts its own volume name before the same path from the
root, tested. Opened and Normalized agreed for a plain open, an open by another case and an open through a
hard link (which reports the link's own path), and differed only for an open by 8.3 short name, which
Opened keeps -- tested where the volume gives short names, and as agreement where it does not. The NUL
device refuses every form with `ERROR_INVALID_PARAMETER`. A named pipe refuses Normalized
(`ERROR_INVALID_FUNCTION`, or `ERROR_PATH_NOT_FOUND` with the GUID form), and answers Opened only with the
NT form (`\Device\NamedPipe\<name>`) or without a volume (`\<name>`). A path of 376 units, past
`MAX_PATH`, was read whole; renames, a move to another directory, and directories are tested. Seven
sabotages added to [sabotage.json](sabotage.json), all caught. `tools/check-publishable.ps1` could not run
here (the Node module `yaml` is missing) and fails the same way without this change.
