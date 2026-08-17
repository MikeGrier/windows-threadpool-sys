# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M6 — Behavioral-matrix hardening

- [ ] Exercise the raw IOCP backend across the behavioral-matrix cases not yet covered: immediate success under
	`FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`, completion identity under many simultaneous operations, and
	results/payloads retained after native endpoint shutdown. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Decide and document multi-endpoint / multi-threaded drain semantics for a shared `CompletionPort` — who
	drains, and how completions for distinct endpoints are attributed during rundown. See
	[DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Integration test: multi-threaded dequeue and rundown on a port shared by several endpoints.

## M∞ — Horizon (ungated)

- [ ] Operation-family safe adapters (read/write, scatter/gather, socket, `DeviceIoControl`), here or in
	downstream crates, once the generic-submission boundary is settled. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
