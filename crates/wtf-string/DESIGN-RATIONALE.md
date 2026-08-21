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
difference is architectural, and its most defensible payoff (a `Wtf8` arm -- this
crate's `u8`/WTF-8 storage variant, backed by a crate-owned `Vec<u8>` whose WTF-8
storage matches `OsString`'s but is not built on it -- giving a uniform
cross-width API) is built as its own later milestone
([CHECKLIST.md](CHECKLIST.md) M6), not in the first `Wtf16`-only cut.

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

## WTF-8 arm semantics (D-15)

The `Wtf8` width (M6) is the `u8` sibling of the `Wtf16` semantics in D-4/D-8: it
stores arbitrary, ill-formed-tolerant WTF-8 with no validation on construction,
and it owns its encode/decode/compare/format behavior rather than inheriting it
from `OsString` (D-3). `encode_str` is the identity on a UTF-8 `str`'s bytes
(every `str` is already valid WTF-8); exact decode succeeds only for valid UTF-8
(a WTF-8-encoded surrogate or arbitrary bytes yield `None`); ordering and hashing
are binary over the stored bytes; and `Debug` escapes each ill-formed byte
losslessly as `\xNN` so distinct byte inputs stay distinguishable.

The one place the two widths visibly diverge is the **granularity of lossy
decode**, and it is inherited from the std oracle each width delegates to. WTF-16
lossy decode (`String::from_utf16_lossy`) is unit-granular: one ill-formed `u16`
surrogate becomes one U+FFFD. WTF-8 lossy decode (`String::from_utf8_lossy`) is
byte-granular per the Unicode "maximal subpart" rule: a lone surrogate encodes as
three WTF-8 bytes and lossily becomes *three* U+FFFD, not one. This is not a bug
to paper over -- it is the faithful behavior of each width's storage model, and we
specify it as owned behavior (D-15). Consumers that need a canonical replacement
count should compare at the `String`/scalar level after decoding, not by counting
U+FFFD in a lossy view. Cross-width tests therefore assert shared invariants
(checked-decode failure, *full* U+FFFD replacement) rather than raw lossy-string
equality.

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

## OsStr interop is conversion-based (D-14)

The obvious ergonomic wish is `AsRef<OsStr>`, so a `Wtf16Str` could be handed to
any `AsRef<OsStr>`-bound API for free. It cannot exist. On Windows an `OsStr` is
backed by WTF-8 bytes, while `WtfStr<Wtf16>` is backed by `u16` code units;
`AsRef<OsStr>` must return a `&OsStr` borrowing the receiver's own storage, and
there is no `&OsStr` that aliases a `[u16]`. Any bridge therefore has to allocate
and re-encode, which is exactly what `AsRef` promises not to do.

So the interop is spelled as explicit conversions instead: `from_os_str` collects
`OsStrExt::encode_wide` once into owned WTF-16, and `to_os_string` rebuilds an
`OsString` via `OsStringExt::from_wide`. Both are lossless in both directions --
`OsStr` and `WtfStr<Wtf16>` are both WTF supersets, so unpaired surrogates survive
a round trip (`from_os_str(x).to_os_string() == x`). `from_wide` / `encode_wide`
are provided as vocabulary aliases (over `from_units` / the content slice) so a
site currently using `OsString::from_wide` / `OsStrExt::encode_wide` reads the
same after switching to this type. The zero-copy direction that *does* exist --
handing our `u16` slice straight to a wide (`*W`) Win32 call -- is served by
`as_ptr` / `as_terminated_ptr`, not by pretending to be an `OsStr`.

## Safe mutation surface (D-16)

M6's review round flagged a real gap: the design explicitly frames `WtfString`
as reclaiming the "growable" half `widestring` splits into `U16String`
(growable, not terminated), yet the only way to add content after construction
was the `unsafe` FFI buffer-fill protocol (D-9) -- there was no safe way for an
ordinary caller to grow a string at all.

The fix is not a general string-editing API. `std::ffi::OsString` -- the type
this crate is shaped after -- is itself narrow: `push` (append another
`AsRef<OsStr>`), `clear`, and capacity management (`reserve` /
`reserve_exact` / `shrink_to_fit` / `shrink_to`). It has no `truncate`, `pop`,
or indexed edit, because `OsStr`'s content is opaque code units whose
encoding varies by platform (WTF-8 here, arbitrary bytes on Unix): an
arbitrary byte-offset truncation could land mid-sequence and produce
ill-formed content with no way to detect it after the fact. `WtfStr<E>`'s
content is exactly as opaque -- ill-formed WTF-16/WTF-8 is a first-class,
supported value (D-4/D-15) -- so the same restriction applies for the same
reason, and matching `OsString`'s actual surface (rather than `String`'s
richer one) is the correct scope, not an arbitrary cut-down.

`push`/`push_str` and `clear` all re-establish the always-present terminator
(D-7) as their last step, so the invariant that lets `as_terminated_ptr` stay
allocation-free never lapses between mutating calls.

## The PCWSTR parameter seam (D-17)

The crate's whole value proposition is that a wide Win32 call costs no
conversion and no allocation (D-9/D-10). Raw `windows-sys` signatures take
`*const u16`, so `as_terminated_ptr` already satisfies them. The high-level
`windows` crate does not: it spells the same parameter as
`impl Param<PCWSTR>`, and a bound is not something a caller can satisfy by
having the right pointer. Without an impl, a `windows` user has to convert --
typically through `HSTRING` or a fresh `Vec<u16>` -- which is exactly the cost
this crate exists to remove. So the seam is implemented (M8), and gated behind
an off-by-default feature so the zero-dependency default build (D-11) is
untouched.

Two constraints came out of building it. Both are accepted rather than worked
around, because neither has a fix that is ours to make.

**It is pinned to one `windows-core` version.** `PCWSTR` is a concrete type
from a specific `windows-core` release, so `impl Param<PCWSTR> for
&Wtf16String` only applies to callers who resolve to that same
semver-compatible version. A caller on a different major line gets a
*different* `PCWSTR` and the impl silently does not apply to them. This is
inherent to implementing a foreign trait for a foreign type -- there is no way
to be version-agnostic -- and it means bumping the `windows-core` dependency is
a breaking change for feature users, not a routine update. That is why the
dependency is an exact-ish pin and why the feature is named after the crate it
binds to, leaving room for parallel `windows-core-0_xx` features later if
supporting two lines at once ever becomes necessary.

**It binds to `#[doc(hidden)]` machinery.** `Param`'s only method is
`#[doc(hidden)] unsafe fn param`, its `ParamValue` return type and the
`Type`/`TypeKind` bounds are all `#[doc(hidden)]`, and windows-rs documents the
trait as "There is no need to implement this trait." That guidance is aimed at
users of generated bindings, for whom the blanket impls suffice; for an outside
string type there is no blanket impl and no other way to satisfy the bound. So
this is the one place the crate knowingly binds to another layer's *unspecified*
surface, which the workspace's platform rules otherwise forbid. The exposure is
contained deliberately: it lives in one feature-gated module, the impl body is
a single pointer wrap that mirrors `&HSTRING`'s own impl verbatim, and its
observable behaviour -- that the callee receives our exact terminated pointer --
is pinned by tests including a real `lstrlenW` call. An upstream change to the
hidden machinery breaks the build loudly at the impl, not silently at a call
site.

**`&Wtf16Str` deliberately gets no impl.** A borrowed slice carries no
terminator (D-7), so there is no valid `PCWSTR` to hand over; providing one
would mean handing a callee a pointer it would read past the end of. Borrowed
content reaches Win32 through the counted `as_ptr` + `len` pair instead. The
same reasoning caps what the owned impl can promise: the conversion is
infallible, so a value with an interior NUL is seen truncated by the callee.
That is not a new hazard -- it is precisely the C-string caveat of the pointer
being wrapped, and `&HSTRING`'s impl behaves identically -- but it is pinned by
a test so the behaviour cannot drift unnoticed.

## The no_std baseline (D-18)

D-5 already committed to a portable core: storage and the `str` conversions use
only `core`/`alloc` facilities (`str::encode_utf16`, `char::decode_utf16`,
`String::from_utf16[_lossy]`, `str::utf8_chunks`), and only the `OsStr` interop
is platform-gated. M9 makes that claim structural rather than incidental. The
crate root is unconditionally `#![no_std]` with `extern crate alloc`, so the
portable core cannot quietly acquire a `std` dependency: it would stop
compiling.

The `std` feature is therefore **additive and narrow**. It adds exactly one
thing -- the Windows `OsStr`/`OsString` interop -- because `OsStr` lives in
`std` and, unlike `String`/`Vec`, has no `alloc`-only equivalent to fall back
on. Everything else the crate offers, including the mutation surface (D-16) and
the whole FFI pointer surface (D-9), is available `alloc`-only. The feature is
on by default so the ordinary Windows user sees no change; turning it off is
what an embedded or `alloc`-only consumer does.

Two linkage details are worth recording because they are easy to get wrong.
`std` is also linked under `cfg(test)`, since the tests are std-only (they use
`HashMap`, `DefaultHasher`, `OsString`); the library itself never is. And it is
linked under `cfg(doc)`, because the docs link to `std::ffi::OsStr` and
`std::ffi::OsString` throughout to explain what this crate is an analog *of* --
without that, an `alloc`-only documentation build would fail the workspace's
deny-broken-intra-doc-links rule.

**How `no_std`-ness is proven matters more than the claim.** Running
`--no-default-features` on a normal host target is weak evidence: `std` still
exists there, so a dependency that crept back in could still link and the build
would pass. CI therefore builds the crate for a **bare-metal target**
(`thumbv7em-none-eabi`), which ships no `std` at all, so success is a real
proof. The converse was checked too: building that same target *with* the `std`
feature fails at `extern crate std` (E0463), which confirms the gate is
load-bearing rather than vacuous. Because the tests need a harness and are
std-only, the alloc-only *behaviour* is exercised by a host test run of the same
feature configuration; the target build covers `no_std`-ness, the host run
covers correctness.

