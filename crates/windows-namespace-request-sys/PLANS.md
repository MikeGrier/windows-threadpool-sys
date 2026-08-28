# Plans: windows-namespace-request-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

This crate's work is planned in a feature-scoped checklist at the workspace root
rather than a local one, because it lands alongside a sibling crate and a set of
workspace-level corrections, and the workspace root is their lowest common
source-component. That file is not the workspace
[CHECKLIST.md](../../CHECKLIST.md), which holds unrelated deferred work.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [../../CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) | in progress | M24: foundations -- owned handle duplication, security attributes, path preparation, and the faithful-execution contract. M25: the handle-producing entries. M26: the query entries, closing with an acceptance pass over both operation and scenario coverage. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
