# Completed read/checksum work

## Moved 2026-09-18 20:23:41 -07:00 -- first comparison

### <a id="rc-1"></a>RC-1 -- Implement and capture the isolated read/checksum comparison. *(completed 2026-09-18 20:23:41 UTC-07:00)*

Implemented separate direct-owner, bounded-handoff and independent-direct-worker
arrangements under [DESIGN-NOTES.md](DESIGN-NOTES.md). The implementation shares
file/checksum primitives, not a common scheduler, and records per-block correctness,
resource bounds, CPU/wall timing, latency, pressure, binding and payload-page samples.

The package tests, default-workspace debug/release checks and Clippy gate passed.
[sabotage.json](sabotage.json) was exercised: wrong file offsets, constant checksums
and exaggerated payload peaks failed their corresponding assertions; the equivalent
chunking control passed. OS fault-injection limits are in
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> `RC-D5`.

Raw release evidence and its observations are in the
[capture record](captures/2026-09-19/README.md). All speculative paths are retained
under [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `RC-D6`.

> **-> CROSS-COMPONENT HANDOFF:** next design work returns to `topology-planner` ->
> `MR1` -> `EP-R1.7`; see [CHECKLIST.md](../../CHECKLIST.md).
