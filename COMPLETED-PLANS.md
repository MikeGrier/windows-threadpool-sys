# Completed plans

Checklists whose planned work is complete, across the repository. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that was
finished. Individual milestones are archived in each component's `COMPLETED-CHECKLIST.md`.

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-17 | Workspace metadata, release automation, name reservation, shared cross-crate invariants, generation-stamped operation identities so a retained `OperationId` cannot alias a recycled operation, and four rounds of review hardening: typed wait provenance, teardown-gated re-arming, borrow-checked callback environments, reusable cleanup groups, `stop_and_drain`, and rejection of values the Win32 fields cannot honour. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) | 2026-08-17 | Overlapped-I/O foundation complete: endpoints/provenance, operation storage, raw IOCP and blocking backends, cancellation/rundown, submission seam, and safe per-family adapters for file read/write plus scatter/gather (`fs`), sockets on both backends (`socket`), and `DeviceIoControl` (`device`). | [crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) |
| [crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md) | 2026-08-17 | Thread pool complete: callback environment, private pools, cleanup groups, work, one-shot and periodic timers as distinct types, waits that own a handle of proven provenance, and the `TP_IO` backend over the shared seam, with examples, documentation, and an opt-in timer stress suite. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
