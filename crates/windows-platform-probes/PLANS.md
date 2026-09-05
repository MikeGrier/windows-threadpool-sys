# Plans: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST-claim-word-layout.md](CHECKLIST-claim-word-layout.md) | in progress | Measure how the `reserving_mpsc` claim word's bit apportionment (32/32 vs 16/48) and width (64 vs 128) affect push throughput, so the shipping layout is chosen on evidence. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [../../CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) | in progress | M27: create the crate, migrate this session's probes into it under the three-tier scheme, and queue migration of the nine earlier measurements that still live only in git-ignored scratch. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
