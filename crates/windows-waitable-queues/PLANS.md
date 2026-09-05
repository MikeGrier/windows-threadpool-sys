# Plans: windows-waitable-queues

This crate's work is **not** tracked here. It is introduced by
[CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) at the workspace root, because that effort spans
this crate, a domain runtime, a durability layer, and extensions to three existing crates -- the root is
their lowest common source-component.

This file exists so the per-component plans trackers enumerated in the root
[PLANS.md](../../PLANS.md) are complete, and so that a reader who starts here is sent to the right place
rather than concluding there is no plan.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [../../CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) | in progress | M30 creates this crate and its SPSC shape; M31 adds the bounded-array MPSC, the overflow policies, shutdown, observability, and the contention benchmark that decides whether the deferred shapes are ever built. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
