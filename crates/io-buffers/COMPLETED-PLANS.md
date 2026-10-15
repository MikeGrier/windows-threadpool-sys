# Completed plans: io-buffers

Checklists whose planned work is complete. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that was
finished. Individual milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-10-12 | Buffers for asynchronous I/O, portable: the `IoBuf` and `IoBufMut` contract moved out of `windows-ioring-sys`, adopted by the ring crate, `win-numa-sys` and `windows-overlapped-io-sys`; one owned aligned buffer, `AlignedBuf`, which replaced the aligned allocations in `windows-overlapped-io-sys`, the ring crate's tests and `windows-namespace-request-sys`. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
