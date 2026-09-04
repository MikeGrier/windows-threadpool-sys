# Plans: windows-topology-sys

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | **M6: one record walk, per [D-24](DESIGN-NOTES.md#d-24).** The PR #56 diff review found the crate's two record decoders internally coherent and mutually opposite: `cpu_set` bounded every read and stopped on a bad `Size`; `walk` proved one byte, `assert!`ed on a zero `Size`, and read `GroupCount` x 16 bytes unbounded (up to 1,048,560). The ruling: one shared self-bounding walk, never panic, incoherence recorded in the returned data, and no trust boundary -- the OS is trusted for structural validity, and careful walking is just correct traversal. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |

Completed plans are in [COMPLETED-PLANS.md](COMPLETED-PLANS.md).
