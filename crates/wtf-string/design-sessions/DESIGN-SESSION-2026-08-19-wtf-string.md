# Design session — wtf-string (2026-08-19)

Raw record of the session that produced the crate's initial decisions. Tier-1
[DESIGN-NOTES.md](../DESIGN-NOTES.md) is canonical and wins on any conflict.

**Decisions produced:** D-1 through D-13 (see [DESIGN-NOTES.md](../DESIGN-NOTES.md)).

## Intent (as stated)

`OsString` has the right interface but, on Windows, always pays a conversion
between WTF-8 and (possibly ill-formed) UTF-16, plus allocation, on every wide
API call. For Windows-centric code calling Windows APIs repeatedly, this is a
poor trade. We want the same interface as `OsString` but with native, `u16`,
conversion-free storage — ideally phrased as something specializable by storage
width, so that (in the limit) the `u8` form is `OsString` and the `u16` form is
the new implementation, and consumers pick whichever suits their needs.

## How the discussion went

- **The core insight.** std `OsString` is WTF-8; the win is to store WTF-16
  natively so conversion happens once at the `str`/`OsStr` boundary and never on
  the API hot path. `encode_wide`'s analog becomes a zero-copy borrow. (-> D-4, D-5)

- **The generic-specialization question (the crux).** Rust has no stable
  specialization, and the `u8` arm cannot literally be `std::ffi::OsString`. But
  a `WtfEncoding` trait plus **inherent impls on concrete instantiations** gives
  specialization-shaped APIs (width-specific FFI only on `WtfString<Wtf16>`) with
  no unstable features. Build the seam now, ship only `Wtf16`. (-> D-2, D-3)

- **UTF-8 conversion vs OsString.** `String` is strict UTF-8, so exact conversion
  is fallible for any arbitrary-UTF-16 type; the lossless surrogate-preserving
  property lives in native storage. `Wtf16Str <-> OsStr` is lossless both ways;
  `Wtf16 <-> String` is fallible/lossy. (-> D-8)

- **What Windows takes.** `windows-sys` wide APIs want `*const u16`/`*mut u16`
  (`PCWSTR`/`PWSTR`), terminated or counted. The high-level `windows` crate wraps
  them behind `Param<PCWSTR>`; `HSTRING` is WinRT-only and merely *converts into*
  `PCWSTR`. Win32 output has no owned string type — you fill a buffer or free a
  callee pointer — so a native owned `u16` landing zone is a real gap. (-> D-9, D-10)

- **NUL termination.** Practice adopted: always allocate with a trailing NUL even
  when not requested, so returning a terminated string never re-allocates and
  span callers still get spans that exclude only the trailing terminator (interior
  NULs in content are preserved). The interior-NUL tension is made explicit rather
  than papered over. (-> D-7)

- **Build vs adopt (the real fork).** Stress-tested honestly: `widestring`'s
  `U16String`/`U16CString` already cover the capabilities; only the encoding
  generic is a true delta, and its payoff is deferred. So building is chosen for
  **ownership/evolvability** under the mono-repo philosophy, not a capability
  gap — "at some point in the moderate future we will wish we could just make a
  change to it, and that will be a pain if we don't own the layer." (-> D-1, D-11, D-13)
