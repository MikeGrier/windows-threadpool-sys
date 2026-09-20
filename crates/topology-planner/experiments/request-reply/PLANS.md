# Plans

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](../../CHECKLIST.md) | in progress | EP-X2.2, EP-X2.3 and EP-X2.4 are complete in the parent's [COMPLETED-CHECKLIST.md](../../COMPLETED-CHECKLIST.md#ep-x24). EP-X2.5 is authorized offline as a separate fan-out path under RR-D9. Four paths are retained here: stateless request/reply, keyed state, ordered ingestion, and fan-out. Return to parent EP-R1.7.1 after it. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |

> **CROSS-COMPONENT PREREQUISITE:** parent `topology-planner` -> `EP-X2.2` authorizes
> this offline experiment following completed `read-checksum` -> `EP-X2.1`.
> **-> CROSS-COMPONENT HANDOFF:** return to parent `topology-planner` -> `MR2`
> after EP-X2.3 validation and path disposition. No later experiment is implicitly started.