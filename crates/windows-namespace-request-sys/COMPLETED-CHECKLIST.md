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

## Moved 2026-10-15 17:36:13 -04:00 -- NR-2.2: the runs of disk a volume is made of

### <a id="nr-22"></a>NR-2.2 -- `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` on a volume handle, with the helper's input buffer and its first real pending control. *(completed 2026-10-15 17:36:13 -04:00)*

`QueryVolumeDiskExtents` returns the runs of disk a volume is made of as a `Vec<DiskExtent>`, asks again at the
count the device names when the first buffer is too small, and refuses a reply that does not hold what it counts.
The `DeviceIoControl` helper now takes an input buffer, which `NR-2.3` and `NR-2.4` need, and
`FSCTL_REQUEST_OPLOCK`, which takes input and pends until another handle writes, is the first real control to
reach it through the pending path. The decisions are [D-21](DESIGN-NOTES.md#d-21) and the amended
[D-20](DESIGN-NOTES.md#d-20); how they were reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code
is `0b20b59a`.

Measured here: the system volume reports one run, and a buffer below the count plus one extent is refused with
`ERROR_INVALID_PARAMETER`, where `ERROR_INSUFFICIENT_BUFFER` was expected. A volume that spans disks was not
observed, so the multi-extent retry is tested against a fake device that answers as the real one was measured
to. Thirteen sabotages pin it, in [sabotage.json](sabotage.json), all caught. The checklist's premise that this
control needed the input buffer was wrong (`NR-2.3` and `NR-2.4` do); the buffer was added here regardless, as
scheduled. What this left to do is `NR-2.6`, which now covers two duplicates in `win-volume-sys`, and the
`IOCTL_STORAGE_*` codes of `NR-2.3` and `NR-2.4`, which have not been checked to work through the helper.

## Moved 2026-10-15 17:58:20 -04:00 -- NR-2.3: a disk's device unique identifier

### <a id="nr-23"></a>NR-2.3 -- `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceUniqueIdProperty` on a disk handle, through a shared `storage_property` module. *(completed 2026-10-15 17:58:20 -04:00)*

`QueryStorageDeviceUniqueId` reports a disk's device unique identifier, which names the device and not the slot
it is in, as the bytes the device gave: the header, and the device ID descriptor, device descriptor and drive
layout signature it points to, each delimited from its own header and bounds-checked, none parsed. The query
structure and the learn-the-size loop are in `storage_property`, which `NR-2.4` reuses. The decision is
[D-22](DESIGN-NOTES.md#d-22) and how it was reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code
is `29a02f6e`.

Measured here: the control works through the helper; a buffer of exactly the header gets the header alone, one
between that and the reply gets `ERROR_MORE_DATA` with the header written, and a buffer or input shorter than its
structure is `ERROR_BAD_LENGTH`. The disk reports its device ID descriptor and neither other section. Unlike the
disk-extents entry, the `ERROR_MORE_DATA` half is reachable here, so the loop is tested against the real disk for
every first-buffer size. Twenty-one sabotages pin it, in [sabotage.json](sabotage.json), all caught. Not observed:
a disk that reports the other two sections, and one on another bus. What this left to do is `NR-2.4`, and the
question whether the identifiers inside the descriptors belong in this crate, which `D-22` records as a working
position and not a decision.

## Moved 2026-10-15 18:38:54 -04:00 -- NR-2.7: the generic storage property query

### <a id="nr-27"></a>NR-2.7 -- The generic `IOCTL_STORAGE_QUERY_PROPERTY` entry, `QueryStorageProperty`: any property, any query type, additional parameters, reply as bytes. *(completed 2026-10-15 18:38:54 -04:00)*

Raised by the engineer when `NR-2.3` was reviewed ("Generic property query: yes"; the identifiers inside the
descriptors stay unparsed, "at this time, no. We can revisit later if necessary"). `QueryStorageProperty` is the
same call as `QueryStorageDeviceUniqueId` with the property and the query type chosen by the caller, both as
newtypes over the raw values and neither validated, additional parameters held shared so a copy never allocates,
and the reply returned as exactly the bytes its header says. A query for whether a property exists returns no
structure, so it is one call with no output and an empty reply. The decision is
[D-22](DESIGN-NOTES.md#d-22) and how it was reached, including a probe whose zero bytes wrongly suggested
additional parameters were ignored, is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code is `7aec7e85`.

Measured here: an exists query on a real property succeeds with nothing written, an unknown property is
`ERROR_INVALID_FUNCTION` for both questions, and a mask query and an undefined query type are
`ERROR_NOT_SUPPORTED` for the properties tried, so what a type other than standard and exists returns is not
established. The unique-id property reads the first additional byte and the device property ignores them, which
is how their arrival at a real device is shown. Thirty-three sabotages cover `NR-2.3` and this item together, in
[sabotage.json](sabotage.json), all caught. What this left to do is `NR-2.4`.

## Moved 2026-10-15 19:01:45 -04:00 -- NR-2.4: the NUMA node a disk is attached to

### <a id="nr-24"></a>NR-2.4 -- `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceNumaProperty` on a disk handle, the node carried unaltered. *(completed 2026-10-15 19:01:45 -04:00)*

`QueryStorageDeviceNumaNode` reports the NUMA node a disk is attached to, per disk so that a volume spanning
disks on different nodes reports each (the disks come from `QueryVolumeDiskExtents`) rather than one false
answer. The reply is the three `u32`s of `STORAGE_DEVICE_NUMA_PROPERTY`, and the node is carried as the device
wrote it: the checklist's "documented unknown value" was not found documented, so no sentinel is invented. The
decision is [D-23](DESIGN-NOTES.md#d-23) and how it was reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).
The code is `f46d95fe`.

Two things are stated in the record that the item did not anticipate. The property is documented but its
description names no reply structure and the structure's own reference page was not found; the structure is in
the public SDK header, and `D-23` marks that footing as one the engineer can overrule. And this property answers
a short buffer differently from the unique-id property (`ERROR_INSUFFICIENT_BUFFER` below the header, and the
header alone as a success between the header and the reply), which corrected `storage_property`'s documentation,
which had stated the other property's behaviour as the call's. Measured here: a twelve-byte reply and node zero on
a one-node machine; a node other than zero was not observed. Six sabotages pin it, in
[sabotage.json](sabotage.json), all caught. What this left to do is `NR-2.5` and `NR-2.6`, which is now unblocked.

## Moved 2026-10-15 19:42:00 -04:00 -- NR-2.5: the dependency a virtual disk has on its backing file

### <a id="nr-25"></a>NR-2.5 -- `GetStorageDependencyInformation` (virtdisk) on a volume, root directory or disk handle, with the version as an input and names copied out of the reply. *(completed 2026-10-15 19:42:00 -04:00)*

`QueryStorageDependencyInformation` asks which volumes or disks a volume hosts, or with `HOST_VOLUMES` which host
it: the link from a virtual disk to the file that backs it, for durable-ioring's flush domains. The decision is
[D-24](DESIGN-NOTES.md#d-24) and how it was reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code
is `0306fda0`.

The feature is `Win32_Storage_Vhd`. The call returns its status directly, and one status measured here is an
`NTSTATUS` value (`0xC03A0015`), which is carried raw. The version is an input, and a buffer under 72 bytes is
`ERROR_INVALID_PARAMETER`, so the first size is raised to 72. **No entry was ever observed**: attaching a virtual
disk needs an elevated token, which this machine's account does not have. The entries and names are tested
against replies built from the structure's declared layout and a fake device, 25 sabotages pin them in
[sabotage.json](sabotage.json) (all caught), and `D-24` and the manifest's `notCoveredHere` say what an elevated
run on a machine that can attach a VHD would settle. What this left to do in this crate is `NR-2.6`.
