# Completed plans: windows-waitable-queues

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) | 2026-09-05 | Closed the public-surface gaps found by comparing this crate against the eight most-depended-on Rust queue crates: `pop` now distinguishes an empty queue from a departed producer, `is_full` is on the `Bounded` trait and both handles, and `try_iter`/`drain` are inherent. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
