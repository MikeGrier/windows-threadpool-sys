# wtf-string

`OsString`-shaped strings with native, conversion-free `u16` (WTF-16) storage.

Rust's `OsString` stores WTF-8 on Windows, so every wide (`*W`) Win32 call pays a
WTF-8 ↔ UTF-16 re-encode and an allocation. `wtf-string` stores `[u16]` natively
(WTF-16: arbitrary, ill-formed-surrogate-tolerant UTF-16), so the conversion
happens once at the `str`/`OsStr` boundary and Windows calls are fed with no
conversion and no per-call allocation.

- **Encoding-generic core** — `WtfString<E>` / `WtfStr<E>`; ships both the
  `Wtf16` arm (`Wtf16String` / `Wtf16Str`) and the `Wtf8` arm (`Wtf8String` /
  `Wtf8Str`), a `u8`/WTF-8 storage variant matching `OsString`'s WTF-8 layout.
- **Always-terminated storage** — a hidden trailing NUL makes `LPCWSTR` return
  allocation-free, while content spans exclude only that terminator (interior
  NULs in content are still preserved).
- **Portable core** — storage and `str`/`String` conversions work everywhere;
  only the `OsStr`/`OsString` interop is Windows-only. The crate is `no_std` at
  its root: turn off the default `std` feature and the whole core, including the
  FFI pointer surface, still builds on `alloc` alone.
- **Optional `windows` interop** — the off-by-default `windows-core` feature
  implements `Param<PCWSTR>` for `&Wtf16String`, so high-level `windows` APIs
  accept it directly with no conversion. Without it, the crate has zero
  dependencies.

```rust
use wtf_string::Wtf16String;

// Encode once, at the boundary.
let s = Wtf16String::from("C:\\Windows");

// From here on the units are the storage: no re-encode, no allocation.
assert_eq!(s.len(), 10);
assert_eq!(s.to_string_lossy(), "C:\\Windows");
```

## Conversion costs

The point of the crate is that the middle rows are free; everything that costs
anything is a boundary crossing you asked for.

| Operation | Cost |
|---|---|
| `Wtf16String::from(&str)` / `from(String)` | encode + allocate, once |
| `Wtf16String::from_os_str` (Windows) | encode + allocate, once |
| `as_ptr()` + `len()`, for a counted `*W` call | **free** |
| `as_terminated_ptr()`, for an `LPCWSTR` call | **free** |
| `encode_wide()` (Windows), the `OsStrExt` analog | **free** (borrows our slice) |
| `as_units()` / `len()` / comparison / hashing | **free** |
| `to_string_checked()` / `to_string_lossy()` | decode + allocate |
| `to_os_string()` (Windows) | re-encode + allocate |

`OsString` is the mirror image: its first rows are free and the wide-call rows
re-encode. Which type is right depends on whether your code spends its time in
`str` or in Win32.

## FFI surface

Both directions of a Win32 call are covered without leaving `u16`:

- **Input** — `as_ptr()` + `len()` for counted parameters, or
  `as_terminated_ptr()` for `LPCWSTR`/`PCWSTR`. The terminator is always present
  in the owned buffer, so the terminated form never allocates.
- **Output** — `with_capacity()` / `as_mut_ptr()` / `set_len_from_ffi()` for
  caller-allocated buffer-fill APIs, or `from_wide_ptr()` to copy out of a
  callee-allocated buffer.

[`examples/win32_round_trip.rs`](examples/win32_round_trip.rs) exercises all
three against real kernel32 entry points:

```sh
cargo run --example win32_round_trip
```

## Interior NULs

Content may contain NUL, matching `OsString`. The trailing terminator is
storage, not content: it is excluded from `len()`, `as_units()`, comparison and
hashing. Because a C string ends at the first NUL, a callee reading
`as_terminated_ptr()` sees a value with an interior NUL truncated — counted
access is always exact, and `has_interior_nul()` reports the condition. A
checked no-interior-NUL companion type is a reserved seam for a future release.

Status: the v1 surface is complete and publication-ready; not yet released. See
[CHECKLIST.md](CHECKLIST.md) and [DESIGN-NOTES.md](DESIGN-NOTES.md).

Copyright (c) Mike Grier. Licensed under MIT.
