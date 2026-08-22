# Completed plans: windows-overlapped-io-sys

Checklists whose planned work is complete. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that was
finished. Individual milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-17 | Overlapped-I/O foundation complete: endpoints/provenance, operation storage, raw IOCP and blocking backends, cancellation/rundown, submission seam, safe per-family adapters for file read/write and scatter/gather (`fs`) and sockets on both backends (`socket`), and a buffer-owning but `unsafe` raw-control-code `DeviceIoControl` seam (`device`). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-22 | M10: `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` made usable end to end, closing an orphaned design decision. Adds `NotificationModes` and `UnassociatedEndpoint::set_notification_modes`, carries the mode through association so the submission seam can answer "will a packet arrive", gives `Operation` a synchronous byte-count cell for the out-parameter the adapters previously passed as null, and (breaking) changes every buffer-owning adapter to return a two-state `Started` outcome so a synchronous no-packet completion is reported rather than rejected. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
