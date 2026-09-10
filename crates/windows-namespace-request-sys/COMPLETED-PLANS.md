# Completed plans: windows-namespace-request-sys

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-09-10 | NR-1: pinned the `=X:` arm of drive-relative rooting. Raised as deferred work and completed the same round, because the blocker dissolved on measurement -- `GetFullPathNameW` writes the per-drive entry itself, so a test that sets it adds no hazard. The test now pins verbatim honouring, drive-root fallback, and the write-back. | [DESIGN-NOTES.md](DESIGN-NOTES.md#d-18), [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) |
