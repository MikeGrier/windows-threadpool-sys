# Completed checklist: win-error-codes

## Moved 2026-10-15 21:07:13 -04:00 -- WE-1.1: the crate, with Win32Error

### <a id="we-11"></a>WE-1.1 -- Create the crate, with `Win32Error`, an `io::Error` bridge that is exact for every code, and the workspace plumbing. *(completed 2026-10-15 21:07:13 -04:00)*

`Win32Error` is a `u32`: `from_code` and `code`, `is_success` and `into_result` (zero is
`ERROR_SUCCESS`), and on Windows `to_io_error`, `from_io`, `last` and a `From` impl. The decisions
are [WE-D-1](DESIGN-NOTES.md#we-d-1) to [WE-D-4](DESIGN-NOTES.md#we-d-4). The code is `74dc6da9`.

What the work found beyond the item's text. The `io::Error` bridge is a bit-preserving cast at std's
`i32` boundary, and the tests show it for codes up to `0xFFFFFFFF`, including
`ERROR_VIRTDISK_NOT_VIRTUAL_DISK` (`0xC03A0015`, `-1069940715` to std). The two earlier per-crate
types each failed that: one fell back to `io::Error::other(String)`, one would panic. `from_io`
returns `None` and not zero for an error that carries no code, because zero is `ERROR_SUCCESS`
([WE-D-4](DESIGN-NOTES.md#we-d-4)). The numbers build on every target and the bridge is
`cfg(windows)` ([WE-D-3](DESIGN-NOTES.md#we-d-3)), so the crate joined the portable CI job; a Linux
`check` and `clippy` pass locally, and that job is where its tests run on Linux and macOS. `Display`
shows the code in decimal and hex and, on Windows, std's message, which resolved for the virtdisk
code ("The specified disk is not a virtual disk.") and which carries std's own `(os error N)` suffix,
in signed form for a large code; the crate does not edit it.

The sabotages and a control in [sabotage.json](sabotage.json) pin it, all behaving as declared. Two
are declared `refused-by-build`: the constructor and `into_result` are `const fn` and a const
assertion in the tests evaluates them, so an alteration does not build, which is the stronger rung.
One first attempt, a change of the hex digits' case, was reported inert by the sweep, which compares
text without regard to case, so it was replaced by an unpadded-hex mutation. What this left to do is
`WE-1.2`.

## Moved 2026-10-15 21:14:58 -04:00 -- WE-1.2: LStatus

### <a id="we-12"></a>WE-1.2 -- `LStatus`, an `i32` newtype for the signed carrier, converting to and from `Win32Error` by a lossless bit cast. *(completed 2026-10-15 21:14:58 -04:00)*

`LStatus` is the `LONG` that `LSTATUS` is: `from_raw`, `raw`, `is_success` and `into_result` (which
carries the code as a `Win32Error`), and the two conversions, `to_win32` and `Win32Error::to_lstatus`,
as `const` inherent methods with a `From` impl each. The decision is
[WE-D-5](DESIGN-NOTES.md#we-d-5). The code is `9de86126`.

It is the same number space as `Win32Error` ([WE-D-1](DESIGN-NOTES.md#we-d-1)), so a negative value is
a code above 2^31 and not its magnitude, and the round trip is exact for every value in both
directions. It is not an `Error`, since a value of zero is success. Most of its sabotages are
declared `refused-by-build`, because the methods are `const fn` and a const assertion in the tests
evaluates them with a code that needs the sign bit; the two `From` impls cannot be `const`, so their
sabotages are `caught` by a run. The sample codes the tests share moved to one test-only module, so
the next type reuses the set. What this left to do is `WE-1.3`.

## Moved 2026-10-15 21:34:00 -04:00 -- WE-1.3: HResult, and WE-M1 complete

### <a id="we-13"></a>WE-1.3 -- `HResult`, an `i32` newtype for an `HRESULT`, with `HRESULT_FROM_WIN32`'s rule stated and the `ErrorCode` question decided. *(completed 2026-10-15 21:34:00 -04:00)*

`HResult` has `is_failure` and `is_success`, `into_result`, `code` and `facility` as `winerror.h`'s
macros take them, `is_customer` and `is_ntstatus_mapped`, `from_win32` and `win32`, and `Display` and
`Debug` in hex. The decision is [WE-D-6](DESIGN-NOTES.md#we-d-6). The code is `85a2681b`.

Two things the work found that the item did not say. The facility is 13 bits in the macro, not the 11
the documentation draws: `FACILITY_DXGI`, 0x87A, needs 12, so a test pins it and a sabotage that
narrows the mask to 11 is caught. And durable-ioring's `ErrorCode` already tests the reading this
type takes for `S_OK` (`win32()` is `None`, "S_OK is no error"), so `HResult::win32` follows it and the
round trip is exact for the codes 1 to 65535 and not for zero. The `ErrorCode` question the item left
to be decided here is decided as: not changed by this crate, queued as
[DI-9.7.11](../durable-ioring/CHECKLIST.md) because its `from_win32` and `win32` are a second copy of
this rule and that is a durable-ioring decision.

The `refused-by-build` entries the const assertions give are in [sabotage.json](sabotage.json); one
first attempt failed to match because rustc breaks a long assertion message across lines, so the
assertion was shortened with a named constant.

**`WE-M1` is complete.** Its three items are archived above, and the crate's adoptions are in other
crates ([NR-4.1](../windows-namespace-request-sys/CHECKLIST.md),
[ERR-1](../windows-file-enumeration-sys/CHECKLIST.md),
[WV-2.1](../win-volume-sys/CHECKLIST.md)).
