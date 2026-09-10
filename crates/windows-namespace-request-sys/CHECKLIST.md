# Checklist: windows-namespace-request-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md), and how they were
reached in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). This crate's *creation* is
tracked separately, in the workspace
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestones
M24-M26; that file is feature-scoped and is deleted when its feature completes,
so durable follow-up work for the crate belongs here instead.

## NR-1 -- Pin the per-drive current directory arm of drive-relative rooting

- [ ] **NR-1.1** -- Decide how this crate's tests may control process-global state,
  then pin the `=X:` arm of drive-relative rooting.

  **The gap, stated exactly.** [D-18](DESIGN-NOTES.md#d-18) documents a two-arm
  rule: a drive-relative path like `C:foo` is rooted at *that drive's* recorded
  current directory (the hidden `=C:` entry) for a drive other than the current
  one, while for the current drive the entry is ignored and the process current
  directory wins. `a_drive_relative_path_is_rooted_at_that_drive_and_not_the_process_directory`
  in [tests.rs](src/full_path/tests.rs) pins the second arm and only bounds the
  first: when the chosen drive has no `=X:` entry, an implementation that always
  used the drive root would pass every assertion.

  **Why it was not simply written.** Pinning it needs a controlled `=X:`, and
  both routes cost something this item should decide rather than assume. Setting
  the variable in-process mutates state shared by every test thread, which is
  the hazard [DESIGN-NOTES.md](../windows-file-watcher/DESIGN-NOTES.md) records
  for this workspace's single-process test model. Spawning a child with a
  crafted environment avoids that but makes it an integration test and needs the
  `Win32_System_Environment` feature as a dev-dependency.

  Measured facts the work can rely on, so they are not rediscovered: setting
  `=X:` for a drive that does **not** exist has no effect; setting it for an
  existing non-current drive does move that drive's resolution; and
  `SetCurrentDirectoryW` does not maintain these entries -- a parent shell
  writes them.
