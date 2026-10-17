# Completed plans: windows-namespace-request-sys

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-09-10 | NR-1: pinned the `=X:` arm of drive-relative rooting. Raised as deferred work and completed the same round, because the blocker dissolved on measurement -- `GetFullPathNameW` writes the per-drive entry itself, so a test that sets it adds no hazard. The test now pins verbatim honouring, drive-root fallback, and the write-back. | [DESIGN-NOTES.md](DESIGN-NOTES.md#d-18), [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) |
| [CHECKLIST.md](CHECKLIST.md) | 2026-10-16 | NR-5: measured how `CreateFileW` reads the prefix of a path and recorded it as D-28 (`NR-5.1`), which corrected the reason for D-19 rule 6. The public path-form classifier planned after it (`NR-5.2` to `NR-5.4`) was withdrawn by the engineer the same day, because durable-ioring dropped the early remote refusal it was for. | [DESIGN-NOTES.md](DESIGN-NOTES.md#d-28), [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) |
