# Design notes: wtf-string (Tier 1)

Current, canonical decisions for the crate. This is the authoritative record; the
"why" and the alternatives considered live in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md)
(Tier 2), and the raw design discussion in
[design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md](design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md)
(Tier 3). On any conflict, this file wins.

## Intent

An `OsString`-shaped string whose canonical storage is `[u16]` (**WTF-16**:
arbitrary, ill-formed-surrogate-tolerant UTF-16), so Windows wide (`*W`) APIs are
fed with **no conversion and no per-call allocation**. std's `OsString` stores
WTF-8 and re-encodes to/from UTF-16 on every wide call; this type pays that cost
once, at the boundary from `str`/`OsStr`, and never again. The storage and
`str`/`String` conversions are portable; only the `OsStr`/`OsString` interop is
Windows-specific.

## Decision index

| ID | Decision |
|---|---|
| D-1 | **Build, not adopt.** We re-own this layer rather than depend on [`widestring`](https://crates.io/crates/widestring). This is an **ownership/evolvability** decision, not a capability gap (`widestring` technically suffices today). See [Build vs adopt](DESIGN-RATIONALE.md#build-vs-adopt-d-1). |
| D-2 | Encoding-generic core: `WtfString<E: WtfEncoding>` owns `Vec<E::Unit>`; `WtfStr<E>` is a `#[repr(transparent)]` slice over `[E::Unit]`. Shared API in `impl<E>`; width-specific API via **inherent impls on concrete instantiations** (the specialization-shaped pattern). See [The generic seam](#the-generic-seam). |
| D-3 | v1 ships **only** the `Wtf16` encoding, exposed as `Wtf16String` = `WtfString<Wtf16>` and `Wtf16Str` = `WtfStr<Wtf16>`. The `Wtf8` arm (delegating to std `OsString`) is a designed-in seam, **deferred**, gated on a real byte-storage consumer. |
| D-4 | **WTF-16 semantics:** storage is arbitrary `[u16]`, ill-formed-surrogate-tolerant; construction from units performs **no validation** (mirrors `OsStr`'s WTF-8). |
| D-5 | **Portable core.** Storage and `str` <-> units use std (`str::encode_utf16`, `char::decode_utf16`, `String::from_utf16[_lossy]`); no `cfg(windows)`. Only the `OsStr`/`from_wide`/`encode_wide` interop is behind `cfg(windows)`. |
| D-6 | std parity: `WtfString: Deref<Target = WtfStr>`, plus `AsRef`/`Borrow`/`ToOwned`, `Ord`/`Eq`/`Hash` (binary over units), lossy `Display`, and `OsStr`-style escaped `Debug`. |
| D-7 | **Always-terminated storage.** The owned buffer keeps a trailing `0x0000` beyond the logical length: `len()` and all spans **exclude** it, while a terminated `*const u16` (`LPCWSTR`) is **allocation-free**. Interior NULs are permitted (WTF/`OsString` parity), so the terminated pointer is a valid C string only up to a first interior NUL; `has_interior_nul()` reports the condition, and a checked no-interior-NUL C-string companion may follow. See [Always-terminated storage](#always-terminated-storage). |
| D-8 | Conversions live only at the boundary: `From<&str>`/`From<String>` (encode once), `to_string() -> Result` (fallible exact, strict UTF-8), `to_string_lossy()` (U+FFFD). `Wtf16Str <-> OsStr` is **lossless both ways** on Windows (both are "WTF" supersets); `Wtf16 <-> String` is fallible/lossy because `String` is strict UTF-8. |
| D-9 | **Output constructors** for Win32 output patterns: `with_capacity` + `as_mut_ptr` + `unsafe set_len_from_ffi(units)` (re-establishes the terminator) for caller-allocated buffer-fill APIs; `unsafe from_wide_ptr(ptr, len)` for callee-allocated buffers. This is the native owned landing zone Win32 output lacks. |
| D-10 | FFI fit is `*const u16` (`windows-sys` `PCWSTR`). This is **not** an `HSTRING` analog (WinRT is out of scope). A high-level `windows`-crate `Param<PCWSTR>` impl is a deferred, feature-gated seam so it imposes no dependency. |
| D-11 | **Zero dependencies, std-only** in v1. `widestring` is prior art only. `no_std`/`alloc` support is a deferred seam. |
| D-12 | First consumer: `windows-file-watcher`'s `RelativeName` migrates to `Wtf16Str`/`Wtf16String`, replacing its hand-rolled `Box<[u16]>`. Scheduled in that crate (see its `CHECKLIST.md`), gated on this crate's Windows `OsStr` interop (M5). |
| D-13 | A native-`u16` `Path`/`PathBuf` analog is **out of scope** for this crate. It is the one genuine capability `widestring` lacks; if native-`u16` path manipulation is ever needed it is a separate crate, not this one. |

## Detail

### The generic seam

Rust has no stable template specialization, and the `u8` instantiation cannot
*be* `std::ffi::OsString` (a distinct nominal type). But the C++ intuition still
translates: a `WtfEncoding` trait carries the `Unit` type and the conversion
rules; `impl<E: WtfEncoding>` holds the API common to every width; and
**inherent impls on concrete instantiations** (`impl WtfString<Wtf16> { .. }`)
carry width-specific API such as the `*const u16` FFI surface, which only makes
sense for `Wtf16`. Rust allows inherent methods on a specific instantiation, so
this yields specialization-shaped APIs with no unstable features. v1 implements
only `Wtf16`; `Wtf8` (delegating to std `OsString`) slots into the same seam
later. (D-2, D-3)

### Always-terminated storage

The owned buffer always carries a trailing `0x0000` beyond the logical content,
so returning a NUL-terminated `LPCWSTR` never allocates, while callers who want a
NUL-free span get one (the span excludes the terminator). This deliberately
combines two things `widestring` splits across `U16String` (growable, not
terminated) and `U16CString` (terminated, not growable).

There is one honest tension: WTF-16 content may itself contain an interior
`U+0000` (`OsString` allows it), and a C-string pointer is meaningful only up to
the first NUL. The type therefore keeps interior-NUL *tolerance* (parity) and
exposes `has_interior_nul()`; the terminated-pointer accessor is documented as a
valid C string only when there is no interior NUL. Counted APIs use `as_ptr()`
plus `len()` and are unaffected. (D-7)
