# Completed plans: win-pipes-sys

Checklists whose planned work is complete. A checklist reappears in
[PLANS.md](PLANS.md) if new work is planned against it; the row here stays as the record of the work that was
finished. Individual items are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-10-18 | Memory-safe Rust over the Win32 pipe API, the Learn section *System Services > Interprocess Communications > Pipes > Pipe Reference > Pipe Functions*: named and anonymous pipes, connecting and disconnecting, the handle's info and state, peeking and transacting, waiting on a name, and the queries of who is at each end. The workspace's other sites that called `CreateNamedPipeW` or `ConnectNamedPipe` themselves now use it. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
