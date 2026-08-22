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

- [ ] **M10.4** -- End-to-end coverage: for at least the file and device adapters, set
  skip-on-success on the endpoint, drive an operation that completes synchronously, and assert the adapter
  returns `Started::Completed` with the right bytes and payload, that no packet arrives on the port, and
  that rundown converges immediately (the outstanding count was balanced inline). Pair each with the
  existing non-skip endpoint to assert the same operation returns `Started::Pending` there.

- [ ] **M10.5** -- Record the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md): why `Issued` answers "will a
  packet arrive" rather than "did it finish synchronously" (the distinction that makes an immediate `TRUE`
  a `Pending`), why the adapters return a two-state outcome instead of hiding the synchronous case, and
  why the mode is per-endpoint opt-in rather than a crate default. Update the crate README if it describes
  adapter return shapes.
