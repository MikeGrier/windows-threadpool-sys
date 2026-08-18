# Checklist: windows-overlapped-io-sys

All planned milestones are complete; there are no pending work items. Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

The overlapped-I/O foundation now covers owned endpoints and provenance, pinned operation storage, the raw IOCP
and blocking backends, cancellation and rundown, the submission seam, and safe per-family adapters for every
operation family the design enumerated: file read/write and scatter/gather (`fs`), sockets on both the IOCP and
blocking backends (`socket`), and `DeviceIoControl` (`device`).

Every adapter validates its buffer lengths against the `u32` byte counts the Win32 calls take, rejecting what
cannot be expressed rather than transferring a prefix, and the blocking backend's one-operation-at-a-time
constraint is enforced by `&mut self` rather than documented. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

This file reopens when new work (a new operation family, a new backend, or hardening) is planned.
