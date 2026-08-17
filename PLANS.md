# Plans

Master tracker for every CHECKLIST.md in the repository. Each source-component also keeps its own PLANS.md:
[crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md) and
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | completed | Workspace metadata, release automation, name reservation, shared cross-crate invariants, and generation-stamped operation identities so a retained `OperationId` cannot alias a recycled operation; all milestones complete (archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md)). Reopens for future cross-cutting work. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) | completed | Overlapped-I/O foundation complete: endpoints/provenance, operation storage, raw IOCP and blocking backends, cancellation/rundown, submission seam, and safe per-family adapters for file read/write plus scatter/gather (`fs`), sockets on both backends (`socket`), and `DeviceIoControl` (`device`). Reopens for future work. | [crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) |
| [crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md) | in progress | Thread pool: callback environment, work/timer/wait, and the TP_IO backend over the shared seam. M1–M3 complete (callback environment, work submission, `TP_IO` backend and its behavioral matrix); M4 (safe abstractions and documentation) remains. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
