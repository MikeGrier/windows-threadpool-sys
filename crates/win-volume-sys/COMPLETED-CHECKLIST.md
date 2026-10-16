# Completed checklist: win-volume-sys

## Moved 2026-10-14 22:55:00 -04:00 -- WV-M1: the controls durable-ioring's raw volume backing needs

### <a id="wv-11"></a>WV-11 -- The disks a volume sits on, `volume_disk_extents`. *(completed 2026-10-14 22:55:00 -04:00)*

[WV-D-3](DESIGN-NOTES.md#wv-d-3) and [WV-D-4](DESIGN-NOTES.md#wv-d-4) are the authoritative text.

### <a id="wv-12"></a>WV-12 -- The sector size a volume is addressed in, `bytes_per_sector`. *(completed 2026-10-14 22:55:00 -04:00)*

### <a id="wv-13"></a>WV-13 -- A volume's lock, `lock_volume` and `unlock_volume`. *(completed 2026-10-14 22:55:00 -04:00)*

The tests do not take a lock that succeeds, which would hold a real volume against its users; the
reason is in [sabotage.json](sabotage.json)'s `notCoveredHere`.

## Moved 2026-10-15 21:57:18 -04:00 -- WV-2.1: the helper

### <a id="wv-21"></a>WV-21 -- The overlapped-safe `DeviceIoControl` helper, with an input buffer and exact error codes, replacing `control()`. *(completed 2026-10-15 21:57:18 -04:00)*

`device_control` and its seam `overlapped_call` came from the namespace crate, where they were built for
`NR-2.1` and `NR-2.2`, and `control()` is deleted, so the two copies of the overlapped-and-wait logic
are one. The decision is [WV-D-7](DESIGN-NOTES.md#wv-d-7). The code is `3b5a1fb8`.

Three differences from `control()` are fixes and not only a move. The event is passed with its low bit
set, so a handle on a completion port gets no packet for a call it never made, which `control()` did
not do. A failure's code is read in the statement after the call and reaches the caller exactly,
including a code of 2^31 or more, through `win-error-codes`. And a buffer over 4 GiB is an
`InvalidInput` error and not a panic. `u32_at` and `zeroed_output` came with it; `extents` uses both,
so its allocation that fails is now an error the caller receives.

One result corrected an expectation. The declared blind spot, a control made without its `OVERLAPPED`,
was expected to close once `FSCTL_REQUEST_OPLOCK` was among the tests, because it is the one control
that pends. It did not: the sabotage that drops the `OVERLAPPED` survived the whole suite, and the
manifest says why. Without one, that control blocks inside `DeviceIoControl` until the oplock breaks
and answers the same. The entry stays `survives`, with the measured reason in place of the assumed one.
What this left to do is `WV-2.2`.

## Moved 2026-10-15 22:17:09 -04:00 -- WV-2.2: disk extents

### <a id="wv-22"></a>WV-22 -- `volume_disk_extents` takes the stricter checks of the namespace crate's entry, and `WV-D-3` and `D-21` are one decision. *(completed 2026-10-15 22:17:09 -04:00)*

The two implementations were the same algorithm with the same constants, so what moved was two checks
and the tests that pin them, not a rewrite. A call that claims to have written more than the buffer it
was given is now `InvalidData`, where this function would have sliced past the buffer; and a reply that
needs more room but counts none is asked again with room for one extent, not none, because a smaller
buffer is `ERROR_INVALID_PARAMETER` (measured). The decision is
[WV-D-3](DESIGN-NOTES.md#wv-d-3), which supersedes `D-21` of the namespace crate. The code is `7790bae8`.

Ported from the namespace crate with the checks: the measured refusal of every buffer below one extent
(against the real volume), a refusal that ends the attempts, the count of none, and the oversized byte
count; and sabotage entries for the control code, the first answer's room, the count of none, the
attempt bound, a retried refusal, the oversized byte count and a negative length. The older entry that
recognised `ERROR_MORE_DATA` now mutates `is_more_data`, which reads the code through `win-error-codes`.
The namespace crate's typed `Malformed` and `Unstable` errors were not carried over: both are
`InvalidData` here, as the rest of this crate reports an answer that does not hold what it says. What this
left to do is `WV-2.3`.
