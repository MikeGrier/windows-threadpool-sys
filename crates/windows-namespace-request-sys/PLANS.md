# Plans: windows-namespace-request-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

This crate's work is planned in a feature-scoped checklist at the workspace root
rather than a local one, because it lands alongside a sibling crate and a set of
workspace-level corrections, and the workspace root is their lowest common
source-component. That file is not the workspace
[CHECKLIST.md](../../CHECKLIST.md), which holds unrelated deferred work.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | not started | NR-1: pin the `=X:` arm of drive-relative rooting, once it is decided how this crate's tests may control process-global state. The other arm and the surrounding contract are already pinned; this one is bounded, not pinned. | [DESIGN-NOTES.md](DESIGN-NOTES.md#d-18), [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) |
| [../../CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) | in progress | **This crate's part (M24-M26) is complete**: the foundations (owned handle duplication, security attributes, path preparation, the faithful-execution contract), the four handle-producing entries, the five query entries, a test seam, and an acceptance pass over both operation and scenario coverage. The checklist itself stays open for M27 (`windows-platform-probes`) and the `M26+` items gated on this branch merging with `main` -- including `M26+.3`, the merge-or-delete decision on this crate's duplicated path preparation. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
