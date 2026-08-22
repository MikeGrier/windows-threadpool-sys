# Plans: windows-overlapped-io-sys

Completed checklists are recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they
contained are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M12: close the two gaps M10 and M11 left as prose notes. Adds `IoBuf`/`IoBufMut` for `&'static mut [u8]` (deferred on forbidden "no consumer" grounds), and the socket notification-mode setter behind an IFS-handle capability probe, with `classify_socket` made mode-aware in the same change so the two cannot disagree. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
