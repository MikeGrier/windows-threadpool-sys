# Checklist: win-volume-sys

Memory-safe Rust over the Win32 volume device controls. See [README.md](README.md) for what the crate
is and [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## WV-M1 -- The controls durable-ioring's raw volume backing needs

- [x] **WV-1.1** -- The disks a volume sits on, `volume_disk_extents`. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#wv-11)

- [x] **WV-1.2** -- The sector size a volume is addressed in, `bytes_per_sector`. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#wv-12)

- [x] **WV-1.3** -- A volume's lock, `lock_volume` and `unlock_volume`. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#wv-13)

## WV-M2 -- Take in the storage-locality entries

Opened 2026-10-15 by [WV-D-5](DESIGN-NOTES.md#wv-d-5) and [WV-D-6](DESIGN-NOTES.md#wv-d-6). The
entries built in [windows-namespace-request-sys](../windows-namespace-request-sys/CHECKLIST.md) as
`NR-2.1` to `NR-2.5` and `NR-2.7` are volume and disk operations, so they move here, with their tests,
their sabotage entries, and the decisions that justify them (`D-20` to `D-24` in that crate's
[DESIGN-NOTES.md](../windows-namespace-request-sys/DESIGN-NOTES.md), each given a `WV-D` number here
when its item lands and replaced there by a pointer). **Each item carries its own records**, in the
same commit, so the notes are never a step behind the code.

An item that changes a signature `win-durable-ioring` calls (it uses `volume_disk_extents`,
`bytes_per_sector` and `lock_volume`) **adapts that crate in the same commit**, so the workspace is
never left unbuilt; the commit is a `feat!` of this crate and a `chore` of the consumer, split per
the workspace's release-scope rule where the two do not have to build together.

> **-> CROSS-COMPONENT PREREQUISITE:** `WV-2.1` needs [WE-1.1](../win-error-codes/CHECKLIST.md)
> (`Win32Error`), in `crates/win-error-codes`.

- [x] **WV-2.1** -- The overlapped-safe `DeviceIoControl` helper, with an input buffer and exact error codes, replacing `control()`. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#wv-21)

  **Next:** `WV-2.2`.

- [x] **WV-2.2** -- `volume_disk_extents` takes the stricter checks of the namespace crate's entry, and `WV-D-3` and `D-21` are one decision. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#wv-22)

  **Next:** `WV-2.3`.

- [ ] **WV-2.3** -- **Volume NUMA.** `FSCTL_QUERY_VOLUME_NUMA_INFO` on a file or directory handle.

  **Next:** `WV-2.4`.

- [ ] **WV-2.4** -- **Storage properties.** The shared `IOCTL_STORAGE_QUERY_PROPERTY` mechanics, the
  generic query, the unique-id entry and the disk NUMA entry, with the per-property short-buffer
  table in the module documentation.

  **Next:** `WV-2.5`.

- [ ] **WV-2.5** -- **The dependency query.** `GetStorageDependencyInformation` (virtdisk), with the
  version as an input and names copied out of the reply. Its blind spot comes with it, stated and
  not hidden: no real entry has been observed, because attaching a virtual disk needs an elevated
  token.

  **Next:** [NR-2.8](../windows-namespace-request-sys/CHECKLIST.md), which deletes the originals.

> **-> CROSS-COMPONENT HANDOFF:** next work is in component `windows-namespace-request-sys` ->
> `NR-2` -> `NR-2.8` (delete the storage-locality entries). It must land **before the request
> crate's release**, or the entries are published from the wrong crate. See
> [the order across crates](../durable-ioring/CHECKLIST.md#where-the-open-surface-work-goes-next-read-this-first-when-picking-the-work-back-up).

## M-inf -- Parked

Device controls a volume answers that this crate does not yet wrap, ungated: each graduates to a
numbered milestone when it is taken up.

- [ ] **WV-inf.1** -- **The volume's length** (`IOCTL_DISK_GET_LENGTH_INFO`), so a caller can bound a
  region it allocates inside a volume without asking the file system.

- [ ] **WV-inf.2** -- **Enumerating the machine's volumes** (`FindFirstVolumeW`, `FindNextVolumeW`,
  `FindVolumeClose`), with each volume's disks, so a caller can find which other volumes share a
  disk with its own.

- [ ] **WV-inf.3** -- **The rest of the storage stack** ([WV-D-5](DESIGN-NOTES.md#wv-d-5)):
  partition management, raw partition access, and volume management beyond a lock. Coarse on
  purpose: which calls these are (drive layout, partition table changes, and so on) has not been
  researched, so naming them now would be inventing precision. It graduates to a numbered milestone
  when that research is done, and the research is the first step of it.
