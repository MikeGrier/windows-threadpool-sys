# Checklist: win-error-codes

What the crate is for and what was decided about it: [DESIGN-NOTES.md](DESIGN-NOTES.md). Opened
2026-10-15.

`WE-M1`, `Win32Error`, `LStatus` and `HResult` (`WE-1.1` to `WE-1.3`), and `WE-M2`, `NtStatus`
(`WE-2.1`), are complete and archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Nothing
further is planned in this crate. The work it leads to is in other crates:

> **-> CROSS-COMPONENT HANDOFF:** next work is in components `windows-namespace-request-sys` ->
> `NR-4` -> `NR-4.1`, `windows-file-enumeration-sys` -> `ERR` -> `ERR-1`, and `win-volume-sys` ->
> `WV-M2` -> `WV-2.1`; and durable-ioring's `ErrorCode` helpers are
> [DI-9.7.11](../durable-ioring/CHECKLIST.md). See [the order across crates](../durable-ioring/COMPLETED-CHECKLIST.md#open-surface-order).
