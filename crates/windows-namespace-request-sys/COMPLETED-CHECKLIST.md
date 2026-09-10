# Completed checklists: windows-namespace-request-sys

Append-only. Newest groups at the bottom.

## Moved 2026-09-10 -- NR-1: the per-drive current-directory arm, pinned

### <a id="nr-11"></a>NR-1.1 -- Pin the `=X:` arm of drive-relative rooting. *(completed 2026-09-10 12:24:46 UTC-04:00)*

- [x] **NR-1.1** -- Decide how this crate's tests may control process-global
  state, then pin the `=X:` arm of drive-relative rooting.

  **Completed in the round that raised it, because the blocker turned out not to
  exist.** The item was written to defer the work: pinning the arm needs a
  controlled `=X:`, and the two routes to one -- mutating process-global state
  that other test threads share, or spawning a child process with a crafted
  environment -- looked like a decision about this crate's test shape rather than
  something to take in passing.

  Measurement dissolved the first route's objection within the hour.
  `GetFullPathNameW` **itself** writes the `=X:` entry -- when resolving for a
  drive other than the current one, and when the recorded entry is absent or
  rejected, in which case it is written as the drive root. (An accepted entry is
  left alone, and the current-drive form writes nothing.) The code under test
  therefore already mutates that state on the very path these tests exercise, so
  a test that sets it first introduces no hazard that resolving alone did not,
  and there was nothing left to decide. Isolation across the tests comes from
  their disjoint drive letters.

  `a_drive_relative_path_uses_that_drives_entry_verbatim_and_rewrites_a_bad_one`
  in [drive_entry.rs](src/full_path/tests/drive_entry.rs) now pins all three behaviours: an entry
  naming an existing directory is honoured verbatim (onto a *different* drive,
  which is what makes "that drive's own current directory" a convention rather
  than a guarantee); an entry naming nothing is rejected in favour of the drive
  root; and the call writes the entry back, creating it on a host that had none.
  It draws its drive letter from a list disjoint from every sibling test's, so
  no two can select the same one and race under libtest's thread-per-test model.

  *(Later correction, recorded here because the archive is history and the
  history was briefly wrong: this entry described the selection as a `W`/`U`
  **pair**, matching the helper as written. That helper validated only its first
  letter and returned the second unchecked, so the guarantee the pair implied did
  not hold. The lists are now three letters each, every candidate is checked
  against both the current drive and the probe drive, and the lists themselves
  live in one table in [drive_entry.rs](src/full_path/tests/drive_entry.rs) with a test enforcing
  that they stay disjoint and long enough. This note deliberately does NOT
  enumerate them: an earlier version did, and named five lists after a sixth had
  been added -- so a reader picking letters for a seventh would have consulted
  an inventory missing three of the letters already in use. The table is the
  inventory.)*

  The sibling test keeps its weaker form and now says so: without controlling
  the entry it can only bound the arm.

  *(Later correction: that sibling was named
  `a_drive_relative_path_is_rooted_at_that_drive_and_not_the_process_directory`,
  and the name claimed two things its assertions do not reach -- `ends_with`
  accepts any base, including the process directory, and an accepted entry is
  used verbatim so the result need not be on that drive at all. It is now
  `a_drive_relative_path_carries_its_component_and_the_current_drive_uses_the_process_directory`.)*
