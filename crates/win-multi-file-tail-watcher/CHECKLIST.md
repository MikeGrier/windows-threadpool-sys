# Checklist: win-multi-file-tail-watcher

Reports length changes across a large population of files, on a beat the caller owns. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) for the decisions every item cites, and
[README.md](README.md) for what the crate is.

Each milestone ends in a deliverable exercised by integration tests: every one of them crosses a
real file system, process, or thread-pool boundary. Unit tests accompany each item as usual.

Nothing is pending. `M1` to `M10` are complete and archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md); the two
decisions of `M2+` are parked in [PARKED.md](PARKED.md).
