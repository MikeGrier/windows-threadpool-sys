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
