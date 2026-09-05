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
| [CHECKLIST-api-surface.md](CHECKLIST-api-surface.md) | in progress | Closes the public-surface gaps found by comparing this crate against the eight most-depended-on Rust queue crates: `pop` conflating empty with disconnected, `is_full` missing from the `Bounded` trait, and no `try_iter` or `IntoIterator`. Tracked here rather than at the root because it is scoped entirely to this crate. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
| [../../CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) | in progress | M30 creates this crate and its SPSC shape; M31 adds the bounded-array MPSC, the overflow policies, shutdown, observability, and the contention benchmark that decides whether the deferred shapes are ever built. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
