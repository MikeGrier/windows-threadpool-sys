# Completed plans: windows-platform-probes

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) | 2026-09-05 | Measured how `reserving_mpsc`'s claim word apportionment and width affect throughput, then shipped the layouts as caller-selectable options: `Balanced`, `Enduring`, `Perpetual`, and `Wide` behind the `dwcas` feature. Superseded D-36, whose premise was that fixing SH-14.1 required the claim-protocol replacement. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
