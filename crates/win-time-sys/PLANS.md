# Plans: win-time-sys

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | Safe Rust over the Windows clocks, in two layers -- timelines and the getters that read them -- so the workspace reads its one time base in one place. Adopted by the thread pool and durable-ioring. Parked: the cycle counter, and a clock that colludes with a timed wait. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
