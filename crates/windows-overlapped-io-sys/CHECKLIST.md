# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M7 — Safe file operation adapters (`fs` feature)

- [ ] Add a `pub(crate)` payload-pointer-from-`OVERLAPPED` primitive to `operation.rs` so a family adapter can
	recover its buffer from the pinned operation during submission. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Implement fully-safe synchronous `BlockingEndpoint::read` / `write` behind the `fs` feature, owning the
	buffer and returning `io::Result` with no `unsafe` for the caller. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Implement safe-submission `AssociatedEndpoint::read` / `write` behind `fs` returning a typed `FileIo`
	token whose `claim(&Completion)` safely recovers the buffer and byte count. See
	[DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Integration test (`fs`): a safe file write-then-read round-trip on both the blocking and IOCP backends,
	with no `unsafe` in the test's I/O path.

## M∞ — Horizon (ungated)

- [ ] Safe adapters for the remaining operation families (scatter/gather, socket, `DeviceIoControl`), here or in
	downstream crates, each following the file-family adapter shape. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
