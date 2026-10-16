# win-volume-sys

Memory-safe Rust over the Win32 volume device controls: which disks a volume sits on, what sector it
is addressed in, and its lock. Thin over Win32 and adding no policy -- the `-sys` suffix is that
promise, as the workspace's naming decision defines it.

## What is here

- **`volume_disk_extents`** -- the runs on disks that make up a volume
  (`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`), each a disk number, an offset and a length. A volume on a
  remote system, or a handle that is not a volume, does not answer, so success also shows that a
  handle names a volume on this machine's own disks.
- **`bytes_per_sector`** -- the sector size a volume is addressed in
  (`IOCTL_DISK_GET_DRIVE_GEOMETRY`), which an unbuffered volume handle's transfers must be a multiple
  of and aligned to. A handle to a whole physical disk answers it too (tried on one opened for reading and writing by an administrator).
- **`volume_numa_node`** -- the NUMA node the volume a file, a directory or the volume itself is on
  resides on (`FSCTL_QUERY_VOLUME_NUMA_INFO`), as the device wrote it. A volume that advertises no
  proximity domain answers with its own error, and no value is taken to mean "no node".
- **`storage_device_unique_id`** and **`storage_device_numa`** -- a disk's device unique identifier
  and the NUMA node it is attached to, through `IOCTL_STORAGE_QUERY_PROPERTY`. They take a handle to
  a disk (`\\.\PhysicalDriveN`), opened with no access, and report what the device gave: the
  identifier's sections as bytes, not parsed, and the node unaltered.
- **`query_storage_property`** -- the same control for any property and any kind of query, with
  the reply as the bytes the device gave, so a property this crate has no typed function for is one
  call away.
- **`storage_dependency_information`** -- how a virtual disk relates to the file that backs it
  (`GetStorageDependencyInformation`): for a volume on a VHD, the volume that holds the file, whose
  disks are the ones that matter. The version of the reply is an input, and the names in a version 2
  reply are copied out, so nothing a caller holds points into a buffer. No entry has been observed
  from a real virtual disk, because attaching one needs an elevated token; the entries are tested
  against replies built from the structure's declared layout.
- **`lock_volume`** and **`unlock_volume`** -- `FSCTL_LOCK_VOLUME` and `FSCTL_UNLOCK_VOLUME`: until
  released, or until the handle closes, no other handle can use the volume.

Each takes any handle that lends itself (`AsHandle`), and the first two need no access to the volume
at all: a handle opened with `access_mode(0)` is enough, which an ordinary account can open. Opening
a volume, which `std::fs::OpenOptions` already does for `\\.\C:`, is not here.

```rust
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;

use win_volume_sys::{bytes_per_sector, volume_disk_extents};
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};

// No access to the volume is needed to ask about it.
let volume = OpenOptions::new()
    .access_mode(0)
    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
    .open(r"\\.\C:")?;

let extents = volume_disk_extents(&volume)?;
assert!(!extents.is_empty());
let sector = bytes_per_sector(&volume)?;
assert!(sector.is_power_of_two());
# Ok::<(), std::io::Error>(())
```

## What is not here

- **Opening a volume.** `std::fs::OpenOptions` opens `\\.\C:` and `\\?\Volume{...}`, with the flags a
  caller needs, so this crate adds nothing to it.
- **Any promise about what a volume warrants.** That a volume's disks are shared with another volume,
  or that a lock was taken, says nothing here about durability; the crate that uses these decides
  what follows from them.
- **Overlapped control.** Each call waits for the device, and is the same call whether the handle it
  is lent was opened overlapped or not.

The crate is Windows-only.
