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
  only the `OsStr`/`OsString` interop is Windows-only.
- **Optional `windows` interop** — the off-by-default `windows-core` feature
  implements `Param<PCWSTR>` for `&Wtf16String`, so high-level `windows` APIs
  accept it directly with no conversion. With no features on, the crate has zero
  dependencies.

Status: in development. See [CHECKLIST.md](CHECKLIST.md) and
[DESIGN-NOTES.md](DESIGN-NOTES.md).

Copyright (c) Mike Grier. Licensed under MIT.
