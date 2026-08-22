# Completed plans: windows-threadpool-sys

Checklists whose planned work is complete. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that was
finished. Individual milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-17 | Thread pool complete: callback environment, private pools, cleanup groups whose members the borrow checker protects, work, one-shot and periodic timers as distinct types, waits that own a handle of proven provenance, and the `TP_IO` backend over the shared seam, with examples, generated documentation, and an opt-in timer stress suite. | [../../DESIGN-NOTES.md](../../DESIGN-NOTES.md) |
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-21 | Reopened for M17: a wait target now owns its close routine, so `ThreadpoolWait` can watch a handle released by something other than `CloseHandle` (the `FindCloseChangeNotification` case). Covers both teardown paths -- direct drop and cleanup-group release -- and breaks `WaitableHandle::into_handle`, which now returns `Result<OwnedHandle, Self>` rather than hand out the wrong destructor. Unblocks windows-file-watcher M6.1. | [../../DESIGN-NOTES.md](../../DESIGN-NOTES.md) |
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-21 | Reopened for M18: stop containing callback panics. Every trampoline wrapped its callback in `catch_unwind` and discarded the payload, quietly forgiving violations of the contract's own "must not unwind across the FFI boundary" rule. Removed across all five trampolines, with the six tests that asserted it replaced by a subprocess harness proving the abort. Breaking: a panicking callback now ends the process. | [../../DESIGN-NOTES.md](../../DESIGN-NOTES.md) |
