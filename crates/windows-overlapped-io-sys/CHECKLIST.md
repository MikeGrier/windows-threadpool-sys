# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions
are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

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
