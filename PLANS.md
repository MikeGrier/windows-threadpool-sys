# Plans

Master tracker for every CHECKLIST.md in the repository. Each source-component also keeps its own PLANS.md:
[crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md) and
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | Workspace metadata, release automation, and shared cross-crate invariants. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) | in progress | Overlapped-I/O foundation: safe provenance, gated feature layout, behavioral-matrix hardening, operation-family adapters. | [crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) |
| [crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md) | not started | Thread pool: callback environment, work/timer/wait, and the TP_IO backend over the shared seam. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
