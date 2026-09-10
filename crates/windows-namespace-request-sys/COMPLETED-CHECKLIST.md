# Completed checklists: windows-namespace-request-sys

Append-only. Newest groups at the bottom.

## Moved 2026-09-10 -- NR-1: the per-drive current-directory arm, pinned

### <a id="nr-11"></a>NR-1.1 -- Pin the `=X:` arm of drive-relative rooting. *(completed 2026-09-10)*

- [x] **NR-1.1** -- Decide how this crate's tests may control process-global
  state, then pin the `=X:` arm of drive-relative rooting.

  **Completed in the round that raised it, because the blocker turned out not to
  exist.** The item was written to defer the work: pinning the arm needs a
  controlled `=X:`, and the two routes to one -- mutating process-global state
  that other test threads share, or spawning a child process with a crafted
  environment -- looked like a decision about this crate's test shape rather than
  something to take in passing.

  Measurement dissolved the first route's objection within the hour.
  `GetFullPathNameW` **itself** writes the `=X:` entry on every drive-relative
  resolution, creating it when absent. The code under test already mutates that
  state, so a test that sets it first introduces no hazard that resolving alone
  did not, and there was nothing left to decide.

  `a_drive_relative_path_uses_that_drives_entry_verbatim_and_rewrites_a_bad_one`
  in [tests.rs](src/full_path/tests.rs) now pins all three behaviours: an entry
  naming an existing directory is honoured verbatim (onto a *different* drive,
  which is what makes "that drive's own current directory" a convention rather
  than a guarantee); an entry naming nothing is rejected in favour of the drive
  root; and the call writes the entry back, creating it on a host that had none.
  It uses drive `W` so it cannot race the sibling test's `X`/`Y` under libtest's
  thread-per-test model.

  The sibling test
  `a_drive_relative_path_is_rooted_at_that_drive_and_not_the_process_directory`
  keeps its weaker form and now says so: without controlling the entry it can
  only bound the arm.
