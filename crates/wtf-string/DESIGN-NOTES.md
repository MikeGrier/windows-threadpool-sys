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
| D-3 | The encoding-generic core (D-2) ships the `Wtf16` width first (M1-M5). The `Wtf8` arm -- a `u8`/WTF-8 storage variant whose encode/decode, comparison, and formatting semantics this crate defines, backed by a crate-owned `Vec<u8>` (its WTF-8 storage matches `OsString`'s but the arm is not built on `OsString`; the storage field `Vec<E::Unit>` cannot be specialized to a distinct nominal type per encoding) -- is **implemented** (M6) through the same seam, added without disturbing the shipped `Wtf16` width. Its semantics are [D-15](#decision-index). |
| D-4 | **WTF-16 semantics:** storage is arbitrary `[u16]`, ill-formed-surrogate-tolerant; construction from units performs **no validation** (mirrors `OsStr`'s WTF-8). |
| D-5 | **Portable core.** Storage and `str` <-> units use std (`str::encode_utf16`, `char::decode_utf16`, `String::from_utf16[_lossy]`); no `cfg(windows)`. Only the `OsStr`/`from_wide`/`encode_wide` interop is behind `cfg(windows)`. |
| D-6 | std parity: `WtfString: Deref<Target = WtfStr>`, plus `AsRef`/`Borrow`/`ToOwned`, `Ord`/`Eq`/`Hash` (binary over units), lossy `Display`, and `OsStr`-style escaped `Debug`. |
| D-7 | **Always-terminated storage.** The owned buffer keeps a trailing `0x0000` beyond the logical length: `len()` and all spans **exclude** it, while a terminated `*const u16` (`LPCWSTR`) is **allocation-free**. Interior NULs are permitted (WTF/`OsString` parity), so the terminated pointer is a valid C string only up to a first interior NUL; `has_interior_nul()` reports the condition, and a checked no-interior-NUL C-string companion may follow. See [Always-terminated storage](#always-terminated-storage). |
| D-8 | Conversions live only at the boundary: `From<&str>`/`From<String>` (encode once), `into_string() -> Result` / `to_string_checked() -> Option` (fallible exact, strict UTF-8), `to_string_lossy()` (U+FFFD). `Wtf16Str <-> OsStr` is **lossless both ways** on Windows (both are "WTF" supersets); `Wtf16 <-> String` is fallible/lossy because `String` is strict UTF-8. |
| D-9 | **Output constructors** for Win32 output patterns: `with_capacity` + `as_mut_ptr` + `unsafe set_len_from_ffi(units)` (re-establishes the terminator) for caller-allocated buffer-fill APIs; `unsafe from_wide_ptr(ptr, len)` for callee-allocated buffers. This is the native owned landing zone Win32 output lacks. |
| D-10 | FFI fit is `*const u16` (`windows-sys` `PCWSTR`). This is **not** an `HSTRING` analog (WinRT is out of scope). A high-level `windows`-crate `Param<PCWSTR>` impl is **implemented** (M8) behind the optional `windows-core` feature, so it imposes no dependency when off. Its constraints are [D-17](#decision-index). |
| D-11 | **Zero dependencies** (`widestring` is prior art only). The portable core is `alloc`-friendly; `no_std` / `alloc`-only support is **scheduled** as [CHECKLIST.md](CHECKLIST.md) M9, gating the `std`-only `OsStr` interop behind a default `std` feature. |
| D-12 | First consumer: `windows-file-watcher`'s `RelativeName` migrates to `Wtf16Str`/`Wtf16String`, replacing its hand-rolled `Box<[u16]>`. Scheduled in that crate (see its [CHECKLIST.md](../windows-file-watcher/CHECKLIST.md)), gated on this crate's Windows `OsStr` interop (M5). |
| D-13 | A native-`u16` `Path`/`PathBuf` analog is **out of scope** for this crate. It is the one genuine capability `widestring` lacks; if native-`u16` path manipulation is ever needed it is a separate crate, not this one. |
| D-14 | **Windows `OsStr` interop is conversion-based, not borrowing.** `from_os_str` (encode once) and `to_os_string` (`from_wide`) are lossless both ways (D-8), plus `from_wide` / `encode_wide` vocabulary aliases over our slice. A borrowing **`AsRef<OsStr>` is deliberately not provided**: `OsStr` is WTF-8-backed on Windows while `WtfStr<Wtf16>` is `u16`-backed, so no zero-copy `&OsStr` view of `u16` storage can exist. See [OsStr interop is conversion-based (D-14)](DESIGN-RATIONALE.md#osstr-interop-is-conversion-based-d-14). |
| D-15 | **WTF-8 arm semantics (M6).** `Wtf8` stores arbitrary `[u8]`, ill-formed-WTF-8-tolerant; construction from units performs **no validation** (the `u8` analog of D-4). `encode_str` is the identity on a UTF-8 `str`'s bytes; exact decode is valid-UTF-8-or-`None`; comparison/hash are binary over bytes; `Debug` escapes ill-formed byte runs losslessly as `\xNN`. Lossy decode delegates to `String::from_utf8_lossy`, which is **byte-granular** (one U+FFFD per Unicode maximal-subpart), so a WTF-8-encoded surrogate -- three bytes -- lossily becomes **three** U+FFFD, whereas the `Wtf16` width's unit-granular `String::from_utf16_lossy` yields **one**. Checked-decode failure and full replacement are shared across widths; the U+FFFD **count** is deliberately width-dependent (owned spec, inherited from std per width). See [WTF-8 arm semantics (D-15)](DESIGN-RATIONALE.md#wtf-8-arm-semantics-d-15). |
| D-16 | **Safe mutation surface matches `OsString`'s actual (narrow) surface, not a general string-editing API (M7).** `WtfString<E>::push`/`push_str` append and re-establish the terminator; `clear` truncates to empty and re-establishes it; `capacity`/`reserve`/`reserve_exact`/`shrink_to_fit`/`shrink_to` manage the underlying buffer, always keeping room for the terminator. There is deliberately **no `truncate`/`pop`/indexed edit** -- `OsString` itself has none, because arbitrary truncation could split a multi-byte WTF-8/surrogate sequence into ill-formed content. See [Safe mutation surface (D-16)](DESIGN-RATIONALE.md#safe-mutation-surface-d-16). |
| D-17 | **The `Param<PCWSTR>` seam is version-pinned and binds to `windows-core` internals (M8).** `impl Param<PCWSTR> for &Wtf16String` hands over [`as_terminated_ptr`](src/string.rs) unchanged, so a `windows` call site costs no conversion, allocation or copy. Two constraints are inherent and accepted rather than worked around: (a) the impl names one `windows-core` version's `PCWSTR`, so it applies only to callers resolving to that same semver-compatible version -- a version bump here is a **breaking change** for feature users; (b) `Param`'s only method and its `ParamValue`/`Type`/`TypeKind` supports are `#[doc(hidden)]`, and windows-rs states the trait is not meant to be implemented downstream, so this is the one place the crate deliberately binds to another layer's unspecified surface -- it is confined to this feature-gated module, and its behaviour is pinned by tests that would fail loudly on any upstream change. **No impl is provided for `&Wtf16Str`**: a borrowed slice carries no terminator (D-7), so it has no valid `PCWSTR`; borrowed content uses the counted `as_ptr` + `len` pair. See [The PCWSTR parameter seam (D-17)](DESIGN-RATIONALE.md#the-pcwstr-parameter-seam-d-17). |

## Detail

### The generic seam

Rust has no stable template specialization, and the `u8` instantiation cannot
*be* `std::ffi::OsString` (a distinct nominal type). But the C++ intuition still
translates: a `WtfEncoding` trait carries the `Unit` type and the conversion
rules; `impl<E: WtfEncoding>` holds the API common to every width; and
**inherent impls on concrete instantiations** (`impl WtfString<Wtf16> { .. }`)
carry width-specific API such as the `*const u16` FFI surface, which only makes
sense for `Wtf16`. Rust allows inherent methods on a specific instantiation, so
this yields specialization-shaped APIs with no unstable features. Both widths
now ship: `Wtf16` (`u16` units) and `Wtf8` (`u8`/WTF-8 units whose semantics this
crate owns, backed by a crate-owned `Vec<u8>` that matches `OsString`'s WTF-8
storage but is not built on it -- a `Vec<E::Unit>` field cannot be specialized to a
distinct nominal type per encoding), each carrying only its own width-specific
inherent API (the `*const u16` FFI surface stays on `Wtf16`). (D-2, D-3, D-15)

### Always-terminated storage

The owned buffer always carries a trailing `0x0000` beyond the logical content,
so returning a NUL-terminated `LPCWSTR` never allocates, while callers who want the
content span get one that excludes the trailing terminator (the content may itself
hold interior NULs -- see the tension below). This deliberately
combines two things `widestring` splits across `U16String` (growable, not
terminated) and `U16CString` (terminated, not growable).

There is one honest tension: WTF-16 content may itself contain an interior
`U+0000` (`OsString` allows it), and a C-string pointer is meaningful only up to
the first NUL. The type therefore keeps interior-NUL *tolerance* (parity) and
exposes `has_interior_nul()`; the terminated-pointer accessor is documented as a
valid C string only when there is no interior NUL. Counted APIs use `as_ptr()`
plus `len()` and are unaffected. (D-7)
