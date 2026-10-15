# Checklist: win-volume-sys

Memory-safe Rust over the Win32 volume device controls. See [README.md](README.md) for what the crate
is and [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## WV-M1 -- The controls durable-ioring's raw volume backing needs

- [x] **WV-1.1** -- The disks a volume sits on, `volume_disk_extents`. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#wv-11)

- [x] **WV-1.2** -- The sector size a volume is addressed in, `bytes_per_sector`. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#wv-12)

- [x] **WV-1.3** -- A volume's lock, `lock_volume` and `unlock_volume`. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#wv-13)

## M-inf -- Parked

Device controls a volume answers that this crate does not yet wrap, ungated: each graduates to a
numbered milestone when it is taken up.

- [ ] **WV-inf.1** -- **The volume's length** (`IOCTL_DISK_GET_LENGTH_INFO`), so a caller can bound a
  region it allocates inside a volume without asking the file system.

- [ ] **WV-inf.2** -- **Enumerating the machine's volumes** (`FindFirstVolumeW`, `FindNextVolumeW`,
  `FindVolumeClose`), with each volume's disks, so a caller can find which other volumes share a
  disk with its own.
