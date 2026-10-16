# Completed checklist: windows-file-enumeration-sys

Append-only. Active work is in [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-10-15 23:24:12 -04:00 -- ERR-1: the shared `Win32Error`

### <a id="err-1"></a>ERR-1 -- `Win32Error` is `win_error_codes::Win32Error`, re-exported under the same name. *(completed 2026-10-15 23:24:12 -04:00)*

This crate's own type panicked converting a code of 2^31 or more to an `io::Error` (the virtdisk
errors are such codes), and its `from_io` returned the code `0`, which is `ERROR_SUCCESS`, for an
error with no OS code behind it, so a fabricated error read as success. The shared type does
neither. The decisions are [WE-D-1](../win-error-codes/DESIGN-NOTES.md#we-d-1) and
[WE-D-4](../win-error-codes/DESIGN-NOTES.md#we-d-4). The code is the commit that adds the dependency
and deletes the type from `src/error.rs`.

The re-export keeps every path, and the commit is a breaking one because a caller could see this
change: `from_io` returns `Option<Win32Error>`, `None` for an error with no OS code, where it returned
a `Win32Error` of `0`. Also visible: `to_io_error` no longer panics and the code survives it; `Display`
reads `Win32 error N (0xHHHHHHHH): message` where it read `Win32 error N (message)`; and the type now
implements `std::error::Error` and converts into `io::Error`.

The item said each caller of `from_io` would decide what an error with no code means to it. The one
caller was this crate's own `last`, which reads the thread's last error and so always has a code, and
the shared type has `last` of its own; nothing in the workspace outside the crate used the function.
No sabotage manifest exists for this crate, so the tests carry the change: a code of 2^31 or more is
converted and read back, and an error with no code is `None`.
