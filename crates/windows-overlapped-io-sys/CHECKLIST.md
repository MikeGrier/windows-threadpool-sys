# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M9 — Safe socket operation adapters (`socket` feature)

- [ ] Refactor the IOCP submission core into a shared `CompletionPort::submit_with` helper so both handle and
	socket endpoints reuse the outstanding-operation accounting; `AssociatedEndpoint::submit` delegates to it with
	no behavior change. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Implement the IOCP socket backend behind the `socket` feature: `CompletionPort::associate_socket` and an
	`AssociatedSocket` with `recv` / `send` (`WSARecv` / `WSASend`) returning a typed `SocketIo` token whose
	`claim(&Completion)` recovers the buffer and byte count. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Integration test (`socket`): a loopback TCP send-and-receive round-trip through the IOCP socket adapter,
	with no `unsafe` in the test's I/O path. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

## M∞ — Horizon (ungated)

- [ ] Safe blocking socket backend (`BlockingSocket::recv` / `send`) using the Winsock completion-wait path
	(`WSACreateEvent` in `OVERLAPPED.hEvent` + `WSAGetOverlappedResult`), which differs from the handle blocking
	backend's `GetOverlappedResult`-on-handle wait. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Safe adapters for the `DeviceIoControl` family, here or in downstream crates, following the file-family
	adapter shape. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
