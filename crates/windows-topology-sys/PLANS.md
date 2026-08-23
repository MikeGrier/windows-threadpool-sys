# Plans: windows-topology-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). Completed checklists move to
COMPLETED-PLANS.md and their milestones to COMPLETED-CHECKLIST.md once there are any.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | not started | Safe enumeration of Windows processor, cache, and memory topology, plus a JSON-serializable description that can be discovered or fed in. Exists because the `windows` crate offers only typed FFI here, leaving a walk-by-`Size` over variable-length records with trailing arrays that misreport their own length. Deliberately enumeration rather than a renderer, and deliberately without devices, HMAT attributes, or queue affinity (D-9). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
