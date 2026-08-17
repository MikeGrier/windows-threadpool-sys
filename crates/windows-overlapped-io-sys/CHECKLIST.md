# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M5 — Safe endpoint provenance and feature layout

- [x] Design and implement safe endpoint creators / sealed association to remove reliance on the unsafe
	`assume_overlapped` seam. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Add the gated `windows-sys` feature layout for file, socket, and device operation families, keeping the
	published crate's default feature set minimal. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Integration test: a safe-created endpoint runs a real operation on both the IOCP and blocking backends.

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
