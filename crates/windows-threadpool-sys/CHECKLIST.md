# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md). Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

Milestones M1 (callback environment), M2 (work submission and callback ownership), and M3 (the `TP_IO` backend
over the shared seam) are complete.

## M4 — Safe abstractions and documentation

- [ ] **M4-1** — Implement safe work, timer, wait, and I/O abstractions.

- [ ] **M4-2** — Test callback completion, cancellation, and destruction on Windows.

- [ ] **M4-3** — Add API examples and generated documentation.
