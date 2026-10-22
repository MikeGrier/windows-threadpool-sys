# Checklist: win-volume-sys

Memory-safe Rust over the Win32 volume device controls. See [README.md](README.md) for what the crate
is and [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

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
