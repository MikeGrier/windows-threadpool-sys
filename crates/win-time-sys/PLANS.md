# Plans: win-time-sys

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | Safe Rust over the Windows clocks, in two layers -- timelines and the getters that read them -- so the workspace reads its one time base in one place. Adopted by the thread pool and durable-ioring. Open: the engineer's review of a decision the assistant made while building (WT-M3). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [PARKED.md](PARKED.md) | parked | `WT-inf.1` and `WT-inf.2`: the cycle-counter clock, blocked on having no mapping from cycles to real-world time, and a clock that colludes with a timed wait, gated on the first timed wait that accounts time through a clock. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
