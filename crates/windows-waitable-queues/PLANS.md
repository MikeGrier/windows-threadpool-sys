# Plans: windows-waitable-queues

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md), and completed work
in [COMPLETED-PLANS.md](COMPLETED-PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [../../CHECKLIST.md](../../CHECKLIST.md) | not started | M30.4: re-home `M31.6`, the `loom` verification this crate's design notes reference in three places and its README promises adopters before 1.0, which had no live checklist item anywhere. Scope as [D-29](DESIGN-NOTES.md#d-29) requires -- both MPSC shapes or neither, which is D-29's obligation rather than [D-31](DESIGN-NOTES.md#d-31)'s; D-31 owns only the release timing -- and repoint all five references, which are in [DESIGN-NOTES.md](DESIGN-NOTES.md), [src/doorbell.rs](src/doorbell.rs) and [sabotage.json](sabotage.json). M30.5 also obliges a sweep of this crate's three public promises of verification before 1.0, whichever way that decision goes. The milestone's rationale is in [../../DESIGN-RATIONALE.md](../../DESIGN-RATIONALE.md#machine-checking-what-is-argued), Tier 2, because no decision is taken yet. | [DESIGN-NOTES.md](DESIGN-NOTES.md#d-29), [DESIGN-NOTES.md](DESIGN-NOTES.md#d-31) |
