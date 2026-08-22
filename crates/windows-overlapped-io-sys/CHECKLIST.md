# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions
are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

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

- [ ] **M12.3** -- Cover both: that a `&'static mut [u8]` round-trips through a write and a read with its
  address intact; that the probe accepts an ordinary TCP socket (the base Winsock provider is IFS) and that
  the setter's refusal path returns `Unsupported` rather than a Win32 error; and that a socket in
  skip-on-success mode reports `Started::Completed` with no packet queued and nothing left outstanding,
  paired against a default socket that is always `Pending` -- the same shape as
  `tests/skip_on_success_adapters.rs` uses for files and devices.

- [ ] **M12.4** -- Record in [DESIGN-NOTES.md](DESIGN-NOTES.md): why the socket setter probes rather than
  trusting, and what it probes; why it sits on the associated socket where the handle side sits on the
  unassociated endpoint; the `socket` feature's widening; and that `&'static mut [u8]` is the one shared-
  looking type that *is* a legal read destination, because it is exclusive.
