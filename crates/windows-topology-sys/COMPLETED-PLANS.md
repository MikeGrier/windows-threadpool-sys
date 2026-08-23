# Completed plans: windows-topology-sys

Checklists whose planned work is complete. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that
was finished. Individual milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-22 | M1-M4: safe enumeration of Windows processor, cache, and memory topology (a walk-by-`Size`, trailing-array-respecting wrapper over `GetLogicalProcessorInformationEx`), the open-kinded `Domain`/`Topology` description (including a memory domain with no processors, for CXL-shaped systems), JSON serialization behind a default-off `serde` feature with the schema explicitly not semver-covered, and crate documentation plus a worked example printing the host's topology. Unblocks `windows-ioring-sys`'s `M7` (`ring-copy`). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
