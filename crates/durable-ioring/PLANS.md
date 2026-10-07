# Plans: durable-ioring

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | A ring with durability scheduled into its work, layered on `windows-ioring-sys`'s `IoRing`; planned, no code. The contract ([CONTRACT.md](CONTRACT.md)) and the API shape ([API.md](API.md)) are designed; implementations, the retention layer above this crate, and the flush-failure spike follow. | [DESIGN-NOTES.md](DESIGN-NOTES.md), [DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md) |
