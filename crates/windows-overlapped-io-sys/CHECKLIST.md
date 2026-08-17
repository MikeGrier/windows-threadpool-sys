# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M10 — Safe blocking socket backend (`socket` feature)

- [x] Implement fully-safe `BlockingSocket::recv` / `send` behind the `socket` feature, issuing `WSARecv` /
	`WSASend` with a per-call `WSACreateEvent` completion event and blocking via `WSAGetOverlappedResult`, with no
	`unsafe` for the caller. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Integration test (`socket`): a loopback TCP round-trip through `BlockingSocket`, with no `unsafe` in the
	test's I/O path. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

## M∞ — Horizon (ungated)

- [ ] Safe adapters for the `DeviceIoControl` family, here or in downstream crates, following the file-family
	adapter shape. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
