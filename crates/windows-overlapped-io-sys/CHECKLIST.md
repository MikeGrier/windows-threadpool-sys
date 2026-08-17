# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## M11 — Safe `DeviceIoControl` adapters (`device` feature)

- [x] Implement fully-safe synchronous `BlockingEndpoint::ioctl(code, input, output_len)` behind the `device`
	feature, issuing an overlapped `DeviceIoControl` and returning `io::Result<(Vec<u8>, usize)>` with no `unsafe`
	for the caller. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Implement safe-submission `AssociatedEndpoint::ioctl(code, input, output_len)` behind `device` returning a
	typed `DeviceIoControlIo` token whose `claim(&Completion)` recovers the output buffer and byte count. See
	[DESIGN-NOTES.md](DESIGN-NOTES.md).

- [x] Integration test (`device`): an `FSCTL` query on a real file through both the blocking and IOCP `ioctl`
	adapters, with no `unsafe` in the test's I/O path. See [DESIGN-NOTES.md](DESIGN-NOTES.md).
