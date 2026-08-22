# Plans: windows-overlapped-io-sys

Completed checklists are recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they
contained are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M11: caller-supplied owned buffers. The adapters already transfer buffer ownership and hand it back on completion; they just hardcode `Vec<u8>`, so any caller holding a `Box<[u8]>`, `Arc<[u8]>`, aligned, or pooled buffer pays a conversion copy. Adds `IoBuf`/`IoBufMut` and makes every adapter generic over the buffer, so no adapter copies a caller's bytes by default. Breaking, taken while the crate has no adopters. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
