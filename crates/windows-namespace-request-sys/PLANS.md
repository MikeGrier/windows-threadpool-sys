# Plans: windows-namespace-request-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

This crate's work is planned in a feature-scoped checklist at the workspace root
rather than a local one, because it lands alongside a sibling crate and a set of
workspace-level corrections, and the workspace root is their lowest common
source-component. That file is not the workspace
[CHECKLIST.md](../../CHECKLIST.md), which holds unrelated deferred work.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [../../CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) | in progress | **This crate's part (M24-M26) is complete**: the foundations (owned handle duplication, security attributes, path preparation, the faithful-execution contract), the four handle-producing entries, the five query entries, a test seam, and an acceptance pass over both operation and scenario coverage. The checklist itself stays open for M27 (`windows-platform-probes`) and the `M26+` items gated on this branch merging with `main` -- including `M26+.3`, the merge-or-delete decision on this crate's duplicated path preparation. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
