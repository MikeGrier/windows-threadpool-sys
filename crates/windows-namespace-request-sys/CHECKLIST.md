# Checklist: windows-namespace-request-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md), and how they were
reached in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). This crate's *creation* is
tracked separately, in the workspace
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestones
M24-M26; that file is feature-scoped and is deleted when its feature completes,
so durable follow-up work for the crate belongs here instead.

Completed work is in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## NR-2 -- Storage-locality queries, one entry per call

Opened 2026-10-06 from the durable-ioring design session
([DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md)).
durable-ioring lets a consumer declare each file's **flush domains** -- the disks, caches, virtual
disks or shares its writes depend on
([DI-D-21](../durable-ioring/DESIGN-NOTES.md#di-d-21)) -- and a locality helper (root `M39`) derives
them from Windows' storage topology. That helper sequences the calls below. Each is a catalogue
entry of its own, per [D-3](DESIGN-NOTES.md#d-3) ("one entry per Win32 call"), and reports raw
Win32 outcomes without interpreting them. The file-to-volume hop already exists:
`GetFinalPathNameByHandleW` with `VOLUME_NAME_GUID`. A handle to `\\?\Volume{GUID}` or
`\\.\PhysicalDriveN` comes from the existing `CreateFileW` entry.

- [ ] **NR-2.1** -- **`FSCTL_QUERY_VOLUME_NUMA_INFO`** on a file or directory handle: the NUMA node
  the *volume* resides on, or the documented failure when the device advertises no proximity
  domain. `windows-ioring-sys`' unrun
  [file-handle-numa-spike.rs](../windows-ioring-sys/design-sessions/spikes/file-handle-numa-spike.rs)
  records what is and is not known about it; this machine has one node, so only the failure
  shapes and a node of `0` are testable here.

- [ ] **NR-2.2** -- **`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`** on a volume handle: every disk the
  volume spans, with its extents. A multi-extent volume needs the `ERROR_MORE_DATA` resize loop,
  following the existing variable-size entries.

- [ ] **NR-2.3** -- **`IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceUniqueIdProperty`** on a
  disk handle: the device's `STORAGE_DEVICE_UNIQUE_IDENTIFIER`, its stable identity, variable-sized.
  This, not the disk number, is what a flush-domain key is derived from: a disk number can be
  reused by a different device after hot removal within one process.

- [ ] **NR-2.4** -- **`IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceNumaProperty`** on a disk
  handle: the device's NUMA node, or the documented unknown value. Per disk, so a volume spanning
  disks on different nodes reports each rather than one false answer (the spike's question 6).

- [ ] **NR-2.5** -- **`GetStorageDependencyInformation`** (`virtdisk`) on a virtual disk or volume
  handle: the backing file a VHD or VHDX depends on, so the helper can follow it to its own flush
  domains. Check which `windows-sys` feature carries it. Creating a virtual disk in a test needs
  elevation, so state what the tests can and cannot reach on an unelevated run.

## NR-M-inf -- The horizon (ungated)

- [ ] **NR-inf.1** -- **A request shares the caller's handle as a `SharedHandle`
  instead of duplicating it.** The engineer's direction, 2026-10-07, and a working
  position rather than a decision: this crate should use
  [win-shared-os-owned-handle](../win-shared-os-owned-handle/README.md)'s
  `SharedHandle` (its `SH-1+.1`), and the `DuplicateHandle` capture of
  [D-4](DESIGN-NOTES.md#d-4) and [D-7](DESIGN-NOTES.md#d-7) was non-trivial work
  that is probably not worth it over an `Arc<OwnedHandle>`. **Deferred by the
  engineer because it is not a drop-in replacement**; nothing else gates it. What
  it changes, each to be settled when this is picked up:
  - **Construction.** Capture takes a `BorrowedHandle` from a caller who keeps
    ownership. A shared form needs a `SharedHandle` the caller already holds, and
    a caller with only a borrow cannot make one without giving up its handle.
    Whether duplication survives as a second capture path is open.
  - **D-7's refusals.** Re-check which of the null, `INVALID_HANDLE_VALUE` and
    pseudo-handle refusals still apply to a value that arrives already owned.
  - **Inheritance.** A duplicate is made non-inheritable (D-7); a shared handle
    keeps whatever the opener chose.
  - **Getting the handle back.** `into_owned_handle` and `From<CapturedHandle>
    for OwnedHandle` are infallible today; `SharedHandle::try_into_owned` fails
    while another clone exists. `try_clone` would become infallible.
  - **Unchanged:** D-4's measured state-sharing. Shared or duplicated, the
    request names the caller's kernel object.
  - A breaking change to the published crate.
