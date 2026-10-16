# Completed plans: win-error-codes

Checklists whose planned work is complete. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that was
finished. Individual milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-10-15 | The Win32 error, `LStatus`, `HResult` and `NtStatus` types, one per number space, with an exact `io::Error` round trip for every code, built to replace the per-crate copies that disagreed about a code of 2^31 or more. The crates that adopt them are planned in their own checklists. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
