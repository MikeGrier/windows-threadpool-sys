# Checklist: win-error-codes

**Planned, not built** -- this directory holds a plan and no code, and becomes a crate when `WE-1.1`
begins. What the crate is for and what was decided about it: [DESIGN-NOTES.md](DESIGN-NOTES.md).
Opened 2026-10-15.

## WE-M1 -- The three types

Dependency order. Each item carries its own tests and its own entries in the crate's
`sabotage.json`, per the workspace's sabotage convention, and ends with a `Next:` line.

- [ ] **WE-1.1** -- **Create the crate, with `Win32Error`.** A `u32` newtype: `from_code`, `code`,
  `is_success` and `into_result` (a zero code is `ERROR_SUCCESS`), `from_io` and `to_io_error`
  that are exact for **every** code, a `last()` through `io::Error::last_os_error` so the crate
  needs no `unsafe`, `Display`, and `Error`. The `io::Error` conversion is a bit-preserving cast
  at std's `i32` boundary and nothing else; the codes `0xC03A0015` and above are the tests that
  show it ([D-1](DESIGN-NOTES.md#we-d-1)). The same commit does the creation plumbing so the
  workspace stays green: workspace member, release-please package, the publish trigger that
  `tools/check-publishable.ps1` checks, README, and the portable CI job if the crate is made
  portable (decided here, see [the working position](DESIGN-NOTES.md#working-positions)).

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
