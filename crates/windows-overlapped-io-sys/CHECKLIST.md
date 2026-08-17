# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M8 — Safe scatter/gather file adapters (`fs` feature)

- [x] Add a page-aligned `PageBuffers` type and fully-safe synchronous `BlockingEndpoint::read_scatter` /
	`write_gather` behind the `fs` feature, owning the buffers and the `FILE_SEGMENT_ELEMENT` array. `PageBuffers`
	lands with this, its first consumer. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Implement safe-submission `AssociatedEndpoint::read_scatter` / `write_gather` behind `fs` returning a typed
	`ScatterGatherIo` token whose `claim(&Completion)` recovers the `PageBuffers` and byte count. See
	[DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] Integration test (`fs`): a page-aligned gather-write-then-scatter-read round-trip on both the blocking and
	IOCP backends, with no `unsafe` in the test's I/O path. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

## M∞ — Horizon (ungated)

- [ ] Safe adapters for the remaining operation families (socket, `DeviceIoControl`), here or in downstream
	crates, each following the file-family adapter shape. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
