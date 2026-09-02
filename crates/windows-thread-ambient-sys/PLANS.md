# Plans: windows-thread-ambient-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

This crate's work is planned in a feature-scoped checklist at the workspace root
rather than a local one, because it lands alongside a sibling crate and a set of
workspace-level corrections, and the workspace root is their lowest common
source-component. That file is not the workspace
[CHECKLIST.md](../../CHECKLIST.md), which holds unrelated deferred work.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [../../CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) | in progress | M22: the extraction decision, the WOW64 reclassification, and the per-aspect primitives. M23: the composite -- capture set, capture, guard composition, cross-thread tests, and mutation-gap closure through deterministic test-only fault injection. M24-M26 in the same file cover the sibling `windows-namespace-request-sys` crate. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
