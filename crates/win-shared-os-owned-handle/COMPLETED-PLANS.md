# Completed plans: win-shared-os-owned-handle

Checklists whose planned work is complete. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that was
finished. Individual items are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-10-21 | A shared owner of a Windows handle: std's `OwnedHandle` behind a reference count, which std lacks and crates here had each wrapped under their own name. Published as 0.1.0 on 2026-10-07; the ring crate, `win-durable-ioring` and the other crates that wrapped `Arc<OwnedHandle>` themselves hold a `SharedHandle`, and none is left. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
