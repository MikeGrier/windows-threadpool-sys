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

## Moved 2026-10-18 20:49:59 -04:00 -- PATH: one path contract across the namespace crates

### <a id="path-1"></a>PATH-1 -- This crate takes the request crate's path contract, `AnchoredPath`, and its older copy of the preparation is deleted. *(completed 2026-10-18 20:49:59 -04:00)*

The item, as it was written: consume the namespace request crate's path contract, and delete this crate's older copy
of the preparation. It was the merge-or-delete decision of the workspace checklist's `M26+.3`
([CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md)) and of
[D-9](../windows-namespace-request-sys/DESIGN-NOTES.md#d-9) in the request crate, made on 2026-10-15 at the engineer's
request as: this crate depends on `windows-namespace-request-sys` and its `path.rs` preparation is deleted. What
it named as changing: `D-7`'s `MAX_PATH` cap on an ordinary path goes; a request captures the current directory when
it is built and grafts a clearly safe child onto it; `EnumerationRequest::path()` says what it returns, the path as
given, with a second accessor for the path handed to Win32; an interior NUL is refused. A `feat!` of a published
crate.

What was done. The request crate's `AnchoredPath` was made public to be consumed (its
[D-30](../windows-namespace-request-sys/DESIGN-NOTES.md#d-30)). A request holds one; `path()` is the path as given and
`win32_path()`, new, is what the worker opens. `RequestFailure` loses `EmptyPath`, `PathTooLong`, `NotFullyQualified`
and `PathResolution` and gains `PathCapture`, a failure to read the current directory, with the code when Windows
gave one; so an empty path, a long one and an unqualified `\\?\` path reach the open, and its error is the terminal
`DirectoryOpen`. `src/path.rs` and its tests are deleted. Recorded as [D-22](DESIGN-NOTES.md#d-22), with the rationale
in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The crate's first [sabotage.json](sabotage.json) covers the binding:
the worker opening the path as given is caught only by a child-process test that moves the current directory after
the request is built.
