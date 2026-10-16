# Checklist: win-error-codes

What the crate is for and what was decided about it: [DESIGN-NOTES.md](DESIGN-NOTES.md). Opened
2026-10-15.

## WE-M1 -- The three types

Dependency order. Each item carries its own tests and its own entries in the crate's
`sabotage.json`, per the workspace's sabotage convention, and ends with a `Next:` line.

- [x] **WE-1.1** -- Create the crate, with `Win32Error`, an `io::Error` bridge that is exact for every code, and the workspace plumbing. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#we-11)

  **Next:** `WE-1.2`.

- [ ] **WE-1.2** -- **`LStatus`.** An `i32` newtype for the signed carrier, converting to and from
  `Win32Error` by a bit cast in both directions, with `is_success` and `into_result`.

  **Next:** `WE-1.3`.

- [ ] **WE-1.3** -- **`HResult`.** An `i32` newtype: severity, facility and code accessors,
  `is_failure` and `is_success`, `from_win32` with `HRESULT_FROM_WIN32`'s rule (zero or a value with
  the high bit set is already an `HRESULT`), `win32() -> Option<Win32Error>` for a wrapped Win32
  code, and `Display` as `0x8007045D`. Compare against
  [durable-ioring's `ErrorCode`](../durable-ioring/src/error_code.rs), which has the same
  `from_win32` and `win32` helpers; whether that type is rebuilt on this one is decided here and
  recorded, not left.

  **Next:** the adoptions, in any order, each in its own crate:
  [NR-4.1](../windows-namespace-request-sys/CHECKLIST.md),
  [ERR-1](../windows-file-enumeration-sys/CHECKLIST.md), and
  [WV-2.1](../win-volume-sys/CHECKLIST.md) (which needs `WE-1.1` only).

> **-> CROSS-COMPONENT HANDOFF:** next work is in components `windows-namespace-request-sys` ->
> `NR-4` -> `NR-4.1`, `windows-file-enumeration-sys` -> `ERR` -> `ERR-1`, and `win-volume-sys` ->
> `WV-M2` -> `WV-2.1`. See [the order across crates](../durable-ioring/CHECKLIST.md#where-the-open-surface-work-goes-next-read-this-first-when-picking-the-work-back-up).
