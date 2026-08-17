# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M∞ — Horizon (ungated)

- [ ] Safe blocking socket backend (`BlockingSocket::recv` / `send`) using the Winsock completion-wait path
	(`WSACreateEvent` in `OVERLAPPED.hEvent` + `WSAGetOverlappedResult`), which differs from the handle blocking
	backend's `GetOverlappedResult`-on-handle wait. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Safe adapters for the `DeviceIoControl` family, here or in downstream crates, following the file-family
	adapter shape. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
