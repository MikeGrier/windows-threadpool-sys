# Plans

Master tracker for every CHECKLIST.md in the repository. Each source-component also keeps its own PLANS.md:
[crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md) and
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | completed | Workspace metadata, release automation, name reservation, and shared cross-crate invariants; all current milestones complete (archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md)). Reopens for future cross-cutting work. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) | in progress | Overlapped-I/O foundation: M7 safe file operation adapters (`fs`) in progress; other-family adapters remain. Provenance, gated features, behavioral-matrix and shared-port drain hardening complete. | [crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) |
| [crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md) | not started | Thread pool: callback environment, work/timer/wait, and the TP_IO backend over the shared seam. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
