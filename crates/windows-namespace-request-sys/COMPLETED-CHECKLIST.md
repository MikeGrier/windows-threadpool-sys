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

## Moved 2026-10-13 20:30:00 -04:00 -- NR-3: an allocation that fails is an error the caller receives

### <a id="nr-31"></a>NR-3.1 -- Every allocation of an `AlignedBuf` returns its failure, and `SecurityDescriptor` and `SecurityAttributes` stop being `Clone`. *(completed 2026-10-13 20:30:00 -04:00)*

Raised by the engineer's review of [durable-ioring's DI-9.7.3](../durable-ioring/COMPLETED-CHECKLIST.md#di-973),
which found the first move onto `io_buffers::AlignedBuf` (`IB-2+.4`) had left a panic where there had been an
abort; the engineer rejected both ("I do not find either acceptable"). Done as one change, recorded here as
finished work: `QueryFileInformation::perform` returns `ERROR_NOT_ENOUGH_MEMORY`; `SecurityDescriptor::capture`
has a `SecurityCaptureFailure::Allocate`; `try_clone` replaces `Clone` on `SecurityDescriptor` and
`SecurityAttributes`; a request's `try_clone` reports `HandleCaptureFailure::CopySecurity`. A breaking change to
the published crate. The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) under `D-8`; the sabotages that
pin it, and the one blind spot, are in [sabotage.json](sabotage.json), a manifest this change introduced.
Two of those sabotages -- a copy of an open request that forgets its security attributes -- would have survived
before: nothing tested that a copy kept them, so this change added the tests.

## Moved 2026-10-15 16:13:07 -04:00 -- NR-2.1: where a volume resides, through the one device-control helper

### <a id="nr-21"></a>NR-2.1 -- `FSCTL_QUERY_VOLUME_NUMA_INFO` on a file or directory handle, through the crate's one overlapped-safe `DeviceIoControl` helper. *(completed 2026-10-15 16:13:07 -04:00)*

`QueryVolumeNumaInfo` reports the NUMA node the volume under a file or directory handle resides on, as the
device wrote it; a failure is the device's own code, and a reply whose size is not the structure's is its own
error rather than a read. The call goes through a private helper, so every later control is the same call on an
overlapped handle and on a plain one and posts nothing to a completion port the handle belongs to. The decision
is [D-20](DESIGN-NOTES.md#d-20) and how it was reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code
is `2d6b757a`. `windows-ioring-sys`' unrun
[file-handle-numa-spike.rs](../windows-ioring-sys/design-sessions/spikes/file-handle-numa-spike.rs) records what
was and was not known about this control beforehand.

Measured here: a file on the system volume answers node `0`, and the `NUL` device answers
`ERROR_INVALID_FUNCTION`. This machine has one node, so a node other than `0`, and a volume that advertises no
proximity domain, were not observed. Ten sabotages pin it, in [sabotage.json](sabotage.json). Six cover the entry and the
controls; four cover the helper's pending path, which is driven through its `overlapped_call` seam with an
overlapped `ReadFile` on a named pipe, because no file-system control pends on demand. What this left to do is
`NR-2.2` (an input buffer for the helper, and a control that pends on demand, to exercise `DeviceIoControl`
itself through that path) and `NR-2.6` (the duplicate in `win-volume-sys`).
