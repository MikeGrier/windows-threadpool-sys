# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M∞ — Horizon (ungated)

- [ ] Operation-family safe adapters (read/write, scatter/gather, socket, `DeviceIoControl`), here or in
	downstream crates, once the generic-submission boundary is settled. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
