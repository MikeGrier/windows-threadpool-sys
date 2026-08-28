# Completed plans: windows-file-watcher-example-test-harness

Checklists whose planned work is complete. The row stays as the record of the work that was finished;
individual milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-26 | A published EXAMPLE test harness for file-change-notification handlers, built on windows-file-watcher's `test-util` seam. All six milestones landed: a `Handler` trait and harness-owned serde schedule format (M1), a contract-legal seeded generator (M2), oracles for a panic/invariant violation/wedge (M3), JSON record/replay of a captured pathology (M4), handler-linked `capture`/`replay` bins driving an intentionally-buggy example handler (M5), and three runnable examples plus README/rustdoc exposition and a full-arc integration test (M6). Decisions D-1...D-7 are recorded. A worked exemplar of what a downstream consumer can build on windows-file-watcher's M13 `test-util` seam. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
