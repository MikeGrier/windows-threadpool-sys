# Completed checklist: win-volume-sys

## Moved 2026-10-14 22:55:00 -04:00 -- WV-M1: the controls durable-ioring's raw volume backing needs

### <a id="wv-11"></a>WV-11 -- The disks a volume sits on, `volume_disk_extents`. *(completed 2026-10-14 22:55:00 -04:00)*

[WV-D-3](DESIGN-NOTES.md#wv-d-3) and [WV-D-4](DESIGN-NOTES.md#wv-d-4) are the authoritative text.

### <a id="wv-12"></a>WV-12 -- The sector size a volume is addressed in, `bytes_per_sector`. *(completed 2026-10-14 22:55:00 -04:00)*

### <a id="wv-13"></a>WV-13 -- A volume's lock, `lock_volume` and `unlock_volume`. *(completed 2026-10-14 22:55:00 -04:00)*

The tests do not take a lock that succeeds, which would hold a real volume against its users; the
reason is in [sabotage.json](sabotage.json)'s `notCoveredHere`.
