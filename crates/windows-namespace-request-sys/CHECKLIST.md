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

- [x] **NR-2.1** -- `FSCTL_QUERY_VOLUME_NUMA_INFO` on a file or directory handle, through the crate's one overlapped-safe `DeviceIoControl` helper. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-21)

- [x] **NR-2.2** -- `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` on a volume handle, with the helper's input buffer and its first real pending control. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-22)

- [x] **NR-2.3** -- `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceUniqueIdProperty` on a disk handle, through a shared `storage_property` module. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-23)

- [x] **NR-2.7** -- The generic `IOCTL_STORAGE_QUERY_PROPERTY` entry, `QueryStorageProperty`: any property, any query type, additional parameters, reply as bytes. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-27)

- [x] **NR-2.4** -- `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceNumaProperty` on a disk handle, the node carried unaltered. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-24)

- [x] **NR-2.5** -- `GetStorageDependencyInformation` (virtdisk) on a volume, root directory or disk handle, with the version as an input and names copied out of the reply. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-25)

- [x] **NR-2.6** -- Decided 2026-10-15: the storage-locality entries and their helper belong in `win-volume-sys`, so the duplicate is closed by moving them there and deleting this crate's copies. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-26)

- [x] **NR-2.8** -- The storage-locality entries and the device-control helper are deleted from this crate, and `D-20` to `D-24` are pointers at the `WV-D` rows they became. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-28)

  **Next:** [NR-3.3](#nr-3----what-durable-iorings-open-surface-needs-of-this-crate).

## NR-4 -- Adopt `win-error-codes`

Opened 2026-10-15. This crate has its own `Win32Error`, which converts a code of 2^31 or more to
`io::Error` without keeping the code (the documentation says it survives). See
[WE-D-1](../win-error-codes/DESIGN-NOTES.md#we-d-1).

> **-> CROSS-COMPONENT PREREQUISITE:** [WE-1.1](../win-error-codes/CHECKLIST.md) in
> `crates/win-error-codes`.

- [x] **NR-4.1** -- `Win32Error` is `win_error_codes::Win32Error`, re-exported under the same name, and `ERROR_VIRTDISK_NOT_VIRTUAL_DISK` survives its `io::Error` form. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-41)

## NR-3 -- What durable-ioring's open surface needs of this crate

Opened 2026-10-15 from durable-ioring's `DI-M13` to `DI-M15`. durable-ioring opens its files through
`OpenFile` ([DI-D-63](../durable-ioring/DESIGN-NOTES.md#di-d-63)), so the options it can offer a
consumer are the ones this crate can carry, and some are missing or in the way. All are policy-free,
like the rest of this crate: durable-ioring decides which of them a consumer may use.

**Order, and what comes next.** The order across every crate this touches is at the top of
[durable-ioring's CHECKLIST.md](../durable-ioring/CHECKLIST.md#where-the-open-surface-work-goes-next-read-this-first-when-picking-the-work-back-up);
read it when picking the work back up. Within this crate: `NR-3.3`, then `NR-3.4` to `NR-3.6` as one
commit, and `NR-3.1` and `NR-3.2` independently of both. Each item ends with a `Next:` line.

> **-> CROSS-COMPONENT PREREQUISITE:** [DI-13.5](../durable-ioring/CHECKLIST.md) and
> [DI-13.6](../durable-ioring/CHECKLIST.md) in `crates/durable-ioring` wait on `NR-3.1` and `NR-3.2`;
> `DI-14.3` needs the first and `DI-15.4` the second.

- [x] **NR-3.1** -- `SecurityDescriptor::from_sddl` and `to_sddl`, the SDDL conversion public in both directions. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#nr-31-sddl)

  **Next:** `NR-3.3`.

- [x] **NR-3.2** -- A `ReOpenFile` entry beside `OpenFile` and `OpenFileByIdentifier`, owning a duplicate of the original handle with nothing defaulted. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#nr-32-reopen)

  **Next:** `NR-3.3`; the release of this crate once `NR-3.1` to `NR-3.6` are all done; see the order at the top
  of [durable-ioring's CHECKLIST.md](../durable-ioring/CHECKLIST.md#where-the-open-surface-work-goes-next-read-this-first-when-picking-the-work-back-up).

The path work is **one regularisation of the whole crate**, by the engineer's ruling of 2026-10-15 ("the
whole crate should be regularized"), decided in [D-19](DESIGN-NOTES.md#d-19) and split into
`NR-3.3` to `NR-3.7`. Paths enter this crate at three entries: `OpenFile`, `WatchDirectory` and
`ResolveFullPath`. Every other entry takes a handle, an identifier or a class. All three take a
`Wtf16String`, and none has a length limit, a validation of its own beyond an interior NUL, or a
`\\?\` rewrite.

- [x] **NR-3.3** -- The captured root and the graft, as one internal type (`AnchoredPath`), additive and used by nothing yet. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#nr-33-graft)

  **Next:** `NR-3.4` to `NR-3.6`, as one commit.

- [x] **NR-3.4** -- `OpenFile` and `WatchDirectory` take a `Wtf16String` and hold the graft type, with the path as given and the path handed to Win32 as two accessors. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#nr-34-entries)

- [x] **NR-3.5** -- An interior NUL is refused where a request is built, in every entry that takes a path including `ResolveFullPath`, as a fallible constructor. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#nr-35-nul)

- [x] **NR-3.6** -- `prepare`, `PreparedPath`, `PathError`, `PathFailure` and the `path` module are retired, with the documentation that described them. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#nr-36-retire)

- [x] **NR-3.7** -- The `request_cost` probe in `windows-platform-probes` is rewritten against the new path design, not retired; done with that crate's `M6.1`. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#nr-37-probe)

  **Next:** the release of this crate (a release-please pull request, a new minor), after which
  `NR-3.8` and [DI-13.5 and DI-13.6](../durable-ioring/CHECKLIST.md).

- [ ] **NR-3.8** -- **The enumeration crate's duplicate of the path preparation is resolved.** The
  engineer, 2026-10-15: schedule resolving it. This is the merge-or-delete decision of the workspace
  checklist's `M26+.3` ([CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md)), which was
  gated on this crate's branch merging with `main`, and that has happened. It is made as the first
  option `M26+.3` names: `windows-file-enumeration-sys` consumes the path contract of `NR-3.3` to
  `NR-3.6` and deletes its older copy, [D-9](DESIGN-NOTES.md#d-9)'s reason for duplicating ("that
  crate is released; this one is not") no longer holds, and the layering D-9 recorded puts the
  enumeration crate above this one, so it is the right direction for the dependency. The work is
  `PATH-1` in that crate's checklist; this item is done when that one is, and the workspace item
  `M26+.3` is done with it. Until then, a fix to either copy is applied to both. It waits on the
  release of this crate with `NR-3.3` to `NR-3.6`.
  **Next:** after `PATH-1`, nothing in this crate; see the order at the top of
  [durable-ioring's CHECKLIST.md](../durable-ioring/CHECKLIST.md#where-the-open-surface-work-goes-next-read-this-first-when-picking-the-work-back-up).

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/windows-file-enumeration-sys` ->
  > `PATH` -> `PATH-1`. See [CHECKLIST.md](../windows-file-enumeration-sys/CHECKLIST.md).

  > **-> CROSS-COMPONENT HANDOFF:** when `NR-3.1` to `NR-3.6` are released, the next work is in
  > component `crates/durable-ioring` -> `DI-M13` -> `DI-13.5`, `DI-13.6` and the rework of `DI-13.2`
  > (take the version that carries them). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

## NR-5 -- A path-form classifier: one definition of what a path's syntactic form is

Why this milestone exists. [durable-ioring's `DI-13.1`](../durable-ioring/CHECKLIST.md) ruling (b),
2026-10-16: before an open, a path whose string alone shows it remote is refused, and the full
validation is made on the opened handle only. That syntactic check has to be made on the path
**handed to Win32** ([`OpenFile::win32_path`](src/open.rs)), not on the path as given, because a
relative path under a UNC current directory is grafted into a UNC path. This crate's graft rules
([D-19](DESIGN-NOTES.md#d-19) rules 2, 4 and 6) already classify path forms, privately; a consumer
deriving the same classification would hold a second copy of the rule. The classifier is pure: no OS
call and no policy. It says which documented form a string has, and the consumer decides what that
means.

What this milestone deliberately does not change, each a working position and not a decision:

- The entries keep taking the `windows-sys` types and raw access masks they take now. A typed
  vocabulary for rights, share mode, disposition and attributes belongs to durable-ioring's
  `DI-13.3`, `DI-13.4` and `DI-13.7`, so this crate stays the policy-free catalogue.
- Nothing here bounds a call that blocks. A request is already a value that is built on one thread
  and performed on another, which is the whole answer to a blocked `CreateFileW`; whether
  `CancelSynchronousIo` interrupts a create is not something any design here depends on.
- `perform` keeps taking `&self`, so one captured request can be performed more than once.

- [x] **NR-5.1** -- The decision for the path-form classifier, from measurements, before the type exists: [D-28](DESIGN-NOTES.md#d-28). -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#nr-51-forms)

- [ ] **NR-5.2** -- **`PathForm` and `PathForm::of`**, additive and public, over a `Wtf16Str`,
  specified by the table in [D-28](DESIGN-NOTES.md#d-28): each row of it is pinned by a test, so the
  forms and the UNC marker are the readings `CreateFileW` gives and not `GetFullPathNameW`'s. At
  least ten ordinary cases besides, and every edge the measurements name (empty, a lone separator,
  `\\` alone, `\\?\` alone, `\\?\UNC` with no separator, a lone surrogate in the string, mixed
  slashes). Sabotage entries with a `survives` control, per the usual harness.

- [ ] **NR-5.3** -- **The graft rules 2, 4 and 6 are derived from the classifier**, so a form is
  decided at one site. **Awaiting a ruling from the engineer before this starts:** rule 6 refuses to
  graft onto a `\\?\` or a `\\.\` directory, and [D-28](DESIGN-NOTES.md#d-28) measured that Win32
  normalises a `\\.\` path, so for that prefix the rule is wider than its reason. Either it is derived
  as it stands (no behaviour changes: every existing test in the crate passes unchanged, and the
  corrected reason is the only edit), or it is narrowed to the exactly-backslash verbatim form (a graft
  now applies to a `\\.\` directory, which is a change in behaviour with its own tests). The
  sabotage that shows it is a derivation and not a second copy: changing the classifier's answer for
  one form changes whether a path is grafted.

- [ ] **NR-5.4** -- **`win32_path_form()` on each entry that takes a path** (`OpenFile`,
  `WatchDirectory`, `ResolveFullPath`), classifying the path handed to Win32, so a relative path
  under a UNC current directory reports UNC. Every path-taking entry gets it, since the accessor
  costs nothing and a consumer of any of them needs the same answer. The test for the UNC case is
  the ignored child-process one in [tests/unc_current_directory.rs](tests/unc_current_directory.rs)
  extended, and runs where a UNC current directory can be set.
  **Next:** the release of this crate (a new minor; the commits are `feat(namespace-request)`, one
  crate per commit), then durable-ioring's `DI-13.7`.

  > **-> CROSS-COMPONENT HANDOFF:** when `NR-5.1` to `NR-5.4` are released, the next work is in
  > component `crates/durable-ioring` -> `DI-M13` -> `DI-13.7` (the two-phase open and its syntactic
  > remote refusal). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

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
