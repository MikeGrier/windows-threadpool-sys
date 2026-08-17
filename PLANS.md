# Plans

Master tracker for every CHECKLIST.md in the repository. Each source-component also keeps its own PLANS.md:
[crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md) and
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | completed | Workspace metadata, release automation, name reservation, shared cross-crate invariants, generation-stamped operation identities so a retained `OperationId` cannot alias a recycled operation, and two rounds of review hardening (typed wait provenance, teardown-gated re-arming, borrow-checked callback environments); all milestones complete (archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md)). Reopens for future cross-cutting work. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) | completed | Overlapped-I/O foundation complete: endpoints/provenance, operation storage, raw IOCP and blocking backends, cancellation/rundown, submission seam, and safe per-family adapters for file read/write plus scatter/gather (`fs`), sockets on both backends (`socket`), and `DeviceIoControl` (`device`). Reopens for future work. | [crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) |
| [crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md) | completed | Thread pool complete: callback environment, private pools, cleanup groups, work, one-shot and periodic timers, waits, and the `TP_IO` backend over the shared seam, with examples and generated documentation. Reopens for future work. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
