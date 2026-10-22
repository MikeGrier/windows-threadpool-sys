# Completed plans: win-volume-sys

Checklists whose planned work is complete. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that was
finished. Individual items are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-10-18 | Memory-safe Rust over the Win32 volume device controls: the disks a volume sits on, its sector size and its lock, which durable-ioring's raw volume backing is built on (`WV-M1`), the storage-locality entries taken in from the namespace crate (`WV-M2`), and what telling a volume or a disk from a file needs (`WV-M3`). The calls not yet wrapped are parked in [PARKED.md](PARKED.md). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
