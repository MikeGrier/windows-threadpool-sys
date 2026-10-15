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

## NR-3 -- What durable-ioring's open surface needs of this crate

Opened 2026-10-15 from durable-ioring's `DI-M13` to `DI-M15`. durable-ioring opens its files through
`OpenFile` ([DI-D-63](../durable-ioring/DESIGN-NOTES.md#di-d-63)), so the options it can offer a
consumer are the ones this crate can carry, and some are missing or in the way. All are policy-free,
like the rest of this crate: durable-ioring decides which of them a consumer may use.

> **-> CROSS-COMPONENT PREREQUISITE:** [DI-13.5](../durable-ioring/CHECKLIST.md) and
> [DI-13.6](../durable-ioring/CHECKLIST.md) in `crates/durable-ioring` wait on `NR-3.1` and `NR-3.2`;
> `DI-14.3` needs the first and `DI-15.4` the second.

- [ ] **NR-3.1** -- **A `SecurityDescriptor` from an SDDL string.** This crate owns self-relative
  descriptors, captured from raw pointers, and has no way to make one from text, so a caller wanting
  to create a file with a given security has to build the descriptor itself. A constructor over the
  documented `ConvertStringSecurityDescriptorToSecurityDescriptorW`, freeing what it returns with
  `LocalFree`, giving this crate's own owned type with no raw pointer in the caller's hands. Tests: a
  descriptor from a string reads back the same DACL; an invalid string is the Win32 error, unaltered;
  a descriptor from a string is what `OpenFile::with_security` creates a file with, read back with a
  documented query. A `feat`, released on its own.

- [ ] **NR-3.2** -- **A `ReOpenFile` entry**, beside `OpenFile` and `OpenFileByIdentifier`, one entry
  for one call ([D-3](DESIGN-NOTES.md#d-3)): an owned duplicate of the original handle, as this crate
  does for every handle a request names, with access, share mode and flags, all caller-supplied and
  none defaulted. `perform` returns the new handle and the raw Win32 outcome. Documented to fail on
  an access or sharing that conflicts with a previous open still open, which is the kernel's error
  and passes through. Tests: each documented conflict; a reopen with `FILE_FLAG_NO_BUFFERING` added to
  a handle opened without it. A `feat`, released on its own.

The path work is **one regularisation of the whole crate**, by the engineer's ruling of 2026-10-15 ("the
whole crate should be regularized"), decided in [D-19](DESIGN-NOTES.md#d-19) and split into
`NR-3.3` to `NR-3.7`. Paths enter this crate at three entries: `OpenFile` and `WatchDirectory`, which
take a `PreparedPath` from `prepare` and open it, and `ResolveFullPath`, which already takes a
`Wtf16String` and hands it to `GetFullPathNameW` as it is. Every other entry takes a handle, an
identifier or a class. After this work all three take a `Wtf16String`, and none has a length limit,
a validation of its own, or a `\\?\` rewrite.

- [ ] **NR-3.3** -- **The captured root and the graft, as one internal type.** A type that holds the
  path as the caller gave it and the current directory captured when it was built
  (`GetCurrentDirectoryW`, no I/O), and produces the path to hand to Win32 when the request is
  performed: the child grafted onto the root by concatenation (a backslash unless the root ends in
  one) if all five rules of [D-19](DESIGN-NOTES.md#d-19) hold, and the path unchanged otherwise.
  Nothing is rewritten to a `\\?\` form. Tests: each rule, both ways (a child that meets it is
  grafted, a child that breaks only that rule is not); the graft still names the captured directory
  after the process current directory has changed; a drive root as the captured directory makes no
  doubled backslash; `NUL` grafts and `CON`, `CONIN$` and `CONOUT$` do not; a UNC root with a `..`
  child is not grafted. Replaces `path.rs`'s `prepare`, `resolve` and `validate_verbatim`.

- [ ] **NR-3.4** -- **`OpenFile` and `WatchDirectory` take a `Wtf16String`** and hold the type from
  `NR-3.3`; `OpenFile::path` and `WatchDirectory::path` return the path as it was given, and a second
  accessor returns the path that will be handed to Win32, so a test or a caller can see what the
  graft did. A `feat!` of a published crate. The crate's own tests, doctests and README examples that
  call `prepare` are rewritten.

- [ ] **NR-3.5** -- **An interior NUL is refused in every entry that takes a path, including
  `ResolveFullPath`.** `Wtf16String` does not exclude one (its documentation: a terminated pointer to
  a string with one is *truncated*, and a no-NUL guarantee is "a reserved seam for a future
  release"), so `ResolveFullPath::new` today passes a truncated path to `GetFullPathNameW` without
  saying so, and the `SAFETY` comment in [full_path.rs](src/full_path.rs) that claims "no interior
  NUL by Wtf16String's own invariant" is wrong and is corrected. Open, to be decided: whether the
  constructors become fallible (the assistant's recommendation, as [D-7](DESIGN-NOTES.md#d-7) fails a
  handle that cannot be captured at construction, where the caller can still act) or `perform` reports
  it, which needs an error that is not a raw Win32 code. `ResolveFullPath` stays a faithful entry with
  no graft: it is the Win32 call that resolves against the current directory when performed, and a
  caller who wants a path fixed at construction uses `OpenFile` or `WatchDirectory`.

- [ ] **NR-3.6** -- **Retire `prepare`, `PreparedPath`, `PathError` and `PathFailure`**, and the
  `path` module's public surface, with the documentation that describes them: [lib.rs](src/lib.rs),
  [README.md](README.md), [D-9](DESIGN-NOTES.md#d-9) and [D-18](DESIGN-NOTES.md#d-18) already carry a
  status line pointing at `D-19`. A breaking removal from a published crate.

- [ ] **NR-3.7** -- **The crate's consumers move with it.** `windows-platform-probes`, whose
  `request_cost` probe measures `prepare` and a cloned prepared path and whose premise ("`prepare`
  allocates twice, an input copy and a `MAX_PATH` buffer") no longer holds, so the probe is
  re-measured against the captured root and the graft or retired; and `win-durable-ioring`, whose
  `OpenedFile::open` calls `prepare` today. The assistant's reading of the probe is from its
  comments and has not been checked against its code.

  > **-> CROSS-COMPONENT HANDOFF:** the probe is in `crates/windows-platform-probes`. When
  > `NR-3.1` to `NR-3.7` are released, the next work is in component `crates/durable-ioring` ->
  > `DI-M13` -> `DI-13.5`, `DI-13.6` and the rework of `DI-13.2` (take the version that carries
  > them). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

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
