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
  of and aligned to.
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
