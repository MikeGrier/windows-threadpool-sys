# Checklist: wtf-string

`OsString`-shaped strings with native `u16` (WTF-16) storage. Design and decisions
(D-1…D-13) are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) (Tier 1),
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) (Tier 2), and
[design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md](design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md)
(Tier 3).

Work items are dependency-ordered. Each milestone ends with tests. The implicit
end-of-milestone gate (default build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived to a sibling completed-checklist tracker (created in this directory when
the first group is done).

## M1 — Crate scaffold and design record

- [x] **M1.1** — Scaffold `crates/wtf-string`: portable `Cargo.toml` (no deps, `edition`/`rust-version`/
  `license` from the workspace), `src/lib.rs` crate-doc skeleton, and add the crate to the workspace members.

- [x] **M1.2** — Seed Tier-1 [DESIGN-NOTES.md](DESIGN-NOTES.md), Tier-2 [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md),
  and the Tier-3 session (D-1…D-13); register the crate in the master and crate-local plans. CI covers it
  automatically via the workspace build.

## M2 — Encoding seam and core types

- [x] **M2.1** — `WtfEncoding` trait (associated `Unit`, the `NUL` terminator unit) and the `Wtf16`
  encoding (`Unit = u16`) (D-2/D-3).

- [x] **M2.2** — `WtfString<E>` (owns `Vec<E::Unit>`) and `WtfStr<E>` (`#[repr(transparent)]` over
  `[E::Unit]`): the **always-terminated** storage invariant (hidden trailing `0x0000`; `len()`/spans exclude
  it), construction (`new`, `from_units`), content access on `WtfStr` (`as_units`, `len`, `is_empty`,
  `has_interior_nul`), the `Wtf16String`/`Wtf16Str` aliases, and std parity
  (`Deref`/`AsRef`/`Borrow`/`ToOwned`/`Default`/`Clone`). The FFI pointer surface is M4. (The old M2.2/M2.3
  are merged: the owned type exposes content access *through* `Deref`, so the type definitions and the
  std-parity plumbing cannot compile independently.) (D-4/D-6/D-7)

- [x] **M2.3** — Tests (sibling `tests.rs`): the terminator invariant survives construction/clone; spans
  never include the terminator; interior-NUL content is preserved and `has_interior_nul()` reports it;
  `Deref`/`ToOwned` round-trip; empty and large inputs.

## M3 — str/String conversions and comparison

- [x] **M3.1** — Boundary conversions: `From<&str>` / `From<String>` (encode once), `into_string() -> Result`
  and `to_string_checked() -> Option` (fallible exact — `to_string` is avoided since it would collide with
  `ToString` once `Display` lands in M3.2), and `to_string_lossy()` (U+FFFD) (D-8).

- [x] **M3.2** — `Display` (lossy) and `OsStr`-style escaped `Debug`.

- [x] **M3.3** — `Ord` / `PartialOrd` / `Eq` / `PartialEq` / `Hash` (binary over units), and cross-type
  comparison with `&str` where it is unambiguous.

- [x] **M3.4** — Property tests: round-trips through `str`/`String`, ill-formed surrogates preserved in
  storage and replaced by lossy conversion, `> MAX_PATH` lengths, mixed BMP/astral content.

## M4 — FFI surface

- [ ] **M4.1** — Counted access (`as_ptr` + `len`) and the terminated `LPCWSTR` pointer accessor, documented
  as valid only when `has_interior_nul()` is false (D-7/D-10).

- [ ] **M4.2** — Output constructors: `with_capacity`, `as_mut_ptr`, and `unsafe set_len_from_ffi(units)`
  (re-establishes the terminator) for caller-allocated buffer-fill APIs (D-9).

- [ ] **M4.3** — `unsafe from_wide_ptr(ptr, len)` for callee-allocated buffers, with an explicit safety
  contract (ownership, count semantics, no reference retained) (D-9).

- [ ] **M4.4** — Tests: mock buffer-fill (count-excludes-NUL and count-includes-NUL) rebuilds the invariant;
  `from_wide_ptr` copies losslessly; terminated pointer round-trips through a `from_wide_ptr`.

## M5 — Windows OsStr/OsString interop

- [ ] **M5.1** — `cfg(windows)` lossless bridge: `from_os_str` / `to_os_string` (and `AsRef<OsStr>` where it
  fits) via `encode_wide`/`from_wide`; `from_wide` / `encode_wide`-style helpers that borrow our slice with
  zero copy (D-5/D-8).

- [ ] **M5.2** — Integration test (Windows): lossless `OsStr` -> `Wtf16Str` -> `OsStr` round-trip including
  unpaired surrogates; a real wide Win32 call fed directly from `as_ptr()` with no conversion.

## M6 — Documentation, examples, publication readiness

- [ ] **M6.1** — `README.md` and `lib.rs` top-level docs: the storage model, the conversion-cost contract,
  and the FFI surface.

- [ ] **M6.2** — Runnable example: a wide Win32 round-trip (input via `as_ptr`, output via a buffer-fill
  constructor) showing the zero-conversion path.

- [ ] **M6.3** — Publication readiness: crate metadata, changelog, and a final review pass over the public
  surface; record the deferred seams (below) as reserved.

## M∞ — Horizon (ungated, post-v1)

Parked, not pending: designed-in seams with no numbered milestone. Each graduates when a real consumer
appears. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] **M∞.1** — The `Wtf8` encoding arm: `WtfString<Wtf8>` delegating to std `OsString`, giving a uniform
  API across storage widths (D-3).

- [ ] **M∞.2** — A checked no-interior-NUL C-string companion type (an enforced-guarantee analog of the
  terminated pointer) (D-7).

- [ ] **M∞.3** — Optional, feature-gated `windows`-crate `Param<PCWSTR>` impl so the high-level crate accepts
  our type without a hard dependency (D-10).

- [ ] **M∞.4** — `no_std` / `alloc`-only support (D-11).
