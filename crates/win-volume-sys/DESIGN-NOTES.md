# Design notes: win-volume-sys (Tier 1)

Current canonical decisions for this crate. See [README.md](README.md) for what it is and
[CHECKLIST.md](CHECKLIST.md) for what is planned.

Repository-level decisions this crate sits under: the `win-` prefix and what `-sys` promises, both in
the workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix), and the
repository's rule to call documented APIs only, in
[copilot-instructions.md](../../.github/copilot-instructions.md). The crate was proposed on
2026-10-14 when durable-ioring's raw volume backing
([DI-10.2](../durable-ioring/CHECKLIST.md)) needed the disks a volume sits on, which only a device
control reports, and the engineer chose a new crate to hold the `unsafe` over any existing one.

## Decision index

| ID | Decision |
|---|---|
| <a id="wv-d-1"></a>WV-D-1 | **The crate wraps the device controls a volume answers and nothing it can already be opened for.** The engineer, 2026-10-14: "unsafe home: sounds good, do it". **Here:** `volume_disk_extents` (`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`), `bytes_per_sector` (`IOCTL_DISK_GET_DRIVE_GEOMETRY`), `lock_volume` and `unlock_volume` (`FSCTL_LOCK_VOLUME`, `FSCTL_UNLOCK_VOLUME`). Every one is documented, which the repository's rule requires. **Not here:** opening a volume, because `std::fs::OpenOptions` already opens `\\.\C:` and `\\?\Volume{...}` with whatever flags a caller needs, so a wrapper would add a place to be wrong and nothing else; and any statement about what a volume warrants, which is the using crate's, as the `-sys` promise requires. **Why a crate and not a place inside durable-ioring's driver:** that crate forbids `unsafe` ([DI-D-4](../durable-ioring/DESIGN-NOTES.md#di-d-4)), and the mono-repo rule is to put a call no lower crate wraps in a lower crate. |
| <a id="wv-d-2"></a>WV-D-2 | **Every control takes any handle that lends itself, needs no access for the two queries, and is the same call whether the handle was opened overlapped or not.** A handle opened with `access_mode(0)` can be asked for a volume's extents and sector size, which an account that is not an administrator can open, so the answer that decides a backing's flush domains is not gated on privilege. **The call is made with an `OVERLAPPED` and an event of its own and waited for**, because a device control on an overlapped handle with no `OVERLAPPED` is undefined, and this crate is lent handles whose open mode it cannot inspect (and may not, by the repository's rule: the call that reports it is undocumented). The wait is to completion, so the buffer and the `OVERLAPPED` are never written to after the function returns. A sabotage that drops the `OVERLAPPED` survives, declared: the volumes the tests open take it without incident, so no test can tell the two apart. |
| <a id="wv-d-3"></a>WV-D-3 | **An answer is checked against what it says it holds, and a count the bytes cannot hold is refused.** `volume_disk_extents` reads its count from the header and refuses a count whose extents do not fit in the bytes the device wrote, a negative offset or length, and an answer too short for its header, as `InvalidData`; it never reads outside its buffer. A count that does not fit the first answer is the device's `ERROR_MORE_DATA`, with the count written, and is asked for again at that size, up to a small bound, since a volume whose extents change between calls could otherwise be asked for forever. The first answer has room for one extent at the least, since the device answers `ERROR_INSUFFICIENT_BUFFER`, not `ERROR_MORE_DATA`, below that. A volume with more extents than the first answer holds is a spanned or striped one, which a development machine rarely has, so the retry is tested against a stand-in device. |
| <a id="wv-d-4"></a>WV-D-4 | **A volume that answers `volume_disk_extents` is local.** A volume on a remote system, or a handle that is not a volume, does not answer the control, so success is what shows a handle names a volume on this machine's own disks. The crate says so; what to do with it is the using crate's. |
