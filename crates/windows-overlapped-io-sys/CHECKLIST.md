# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M∞ — Horizon (ungated)

- [ ] Safe adapters for the remaining operation families (scatter/gather, socket, `DeviceIoControl`), here or in
	downstream crates, each following the file-family adapter shape. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
