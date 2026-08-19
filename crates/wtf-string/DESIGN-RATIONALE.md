# Design rationale: wtf-string (Tier 2)

Historical "why" behind the [DESIGN-NOTES.md](DESIGN-NOTES.md) (Tier 1) decisions.
Cross-referenced by decision ID. Tier 1 wins on any conflict; this file is
consulted for reasoning, not for current answers. The raw session is in
[design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md](design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md).

## Build vs adopt (D-1)

We stress-tested "does this collapse because `widestring` suffices?" honestly, and
on **capability** it largely does: `widestring`'s `U16String`/`U16Str`
(growable, counted, ill-formed-tolerant) and `U16CString`/`U16CStr` (terminated,
NUL-free span, one allocation, no interior NUL) already cover native-`u16`
storage, lossless `OsString` interop on Windows, and pointer constructors for
Win32 output. It is mature (1.x, ~100M downloads), permissively licensed (MIT OR
Apache-2.0), and `no_std`-capable.

The one thing it does **not** offer is a single encoding-generic
`WtfString<Encoding>` with per-width inherent impls; it uses macro-duplicated
concrete types (`U16String`, `U32String`, `Utf16String`, ...). But that
difference is architectural, and its most defensible payoff (a `Wtf8` arm — this
crate's `u8`/WTF-8 storage variant, for which std `OsString` is the intended
backing implementation because its WTF-8 storage matches — giving a uniform
cross-width API) is exactly the part
v1 defers.

So the build decision is **not** justified as a capability win. It is an
**ownership** decision, consistent with the mono-repo philosophy of owning every
layer: the expectation is that in the moderate future we will want to make a
change to this type and not have to route it through an upstream. `widestring`
is recorded as prior art we consciously chose to re-own; if the ownership motive
did not apply, adopting it would be the right call.

## UTF-8 conversion, relative to OsString (D-4, D-8)

Rust's `str`/`String` are strict UTF-8 and cannot hold an unpaired surrogate, so
an *exact* conversion to UTF-8 is fundamentally fallible for any arbitrary-UTF-16
type. What differs across designs is the native storage, which decides the cost:

- **std `OsString`** stores WTF-8: `to_str()` is a cheap validity scan (no
  re-encode), but every *Windows* call re-encodes WTF-8 -> UTF-16.
- **This crate / `widestring::U16String`** store `u16`: Windows calls are free,
  but a trip to `String` is a full UTF-16 decode (`into_string() -> Result` /
  `to_string_checked() -> Option`, `to_string_lossy()` with U+FFFD).

The lossless "keep the weird surrogates" property lives in the *native* storage
(WTF-8 for `OsString`, `u16` for us), never in the UTF-8 projection. The bridge
that stays lossless in both directions is `Wtf16Str <-> OsStr` (WTF-16 <-> WTF-8,
both carry lone surrogates) -- that is exactly `encode_wide`/`from_wide`. Only the
`String` projection is fallible/lossy, and that limitation is Rust's, not ours.

## What Windows APIs actually take (D-9, D-10)

**Input.** `windows-sys` wide APIs take bare raw-pointer aliases
`PCWSTR = *const u16` / `PWSTR = *mut u16`, in two conventions: NUL-terminated
(`LPCWSTR`) and counted (ptr + length). The high-level `windows` crate wraps the
same pointers in `PCWSTR`/`PWSTR` newtypes behind a `Param<PCWSTR>` bound; you can
pass an `HSTRING` there, but only as a *conversion into* `PCWSTR` -- the API's
contract is `PCWSTR`, never `HSTRING`. `HSTRING` is the **WinRT** string
(refcounted, immutable, header-prefixed) and is not the type classic kernel32
APIs speak.

**Output.** Classic Win32 has no owned returned-string type: functions fill a
caller buffer (`PWSTR` + length) or hand back a callee-allocated `*mut u16` you
must free. The `windows` crate lets you turn those pointers into a `String`
(fallible/lossy) or an `HSTRING` (WinRT-heavy) -- there is no plain owned `u16`
landing zone. Only WinRT returns a string by value, as `HSTRING`.

Both facts point the same way: a type that (a) stores native `u16` and (b) always
keeps a hidden terminator satisfies every wide signature -- terminated *and*
counted, input *and* output -- with no conversion and no per-call allocation.
That gap (native owned wide string for classic Win32 output) is the crate's most
concrete value, realized by D-9's output constructors. We deliberately do **not**
model `HSTRING`; WinRT interop is out of scope.

## The interior-NUL tension (D-7)

We initially wanted a single growable type that is both interior-NUL-tolerant
(`OsString` parity) and hands out a cheap `LPCWSTR`. Those fight: a terminated
C-string pointer is only meaningful when there is no interior NUL. `widestring`
resolves this with two types (`U16String` tolerant/not-terminated,
`U16CString` no-interior-NUL/terminated). We keep one growable type that is
tolerant *and* always carries a terminator, and make the contract explicit: the
terminated pointer is a valid C string only when `has_interior_nul()` is false;
counted access (`as_ptr` + `len`) is always valid. A dedicated no-interior-NUL
C-string companion can be added later if callers want the guarantee enforced.
