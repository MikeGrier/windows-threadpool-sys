# Completed checklist: wtf-string

Append-only archive of completed milestones moved out of [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-08-19 -- M1 scaffold+design, M2 encoding+core types, M3 conversions/formatting/comparison

### M1 -- Crate scaffold and design record

- [x] **M1.1** -- Scaffold `crates/wtf-string`: portable [Cargo.toml](Cargo.toml) (no deps, `edition`/`rust-version`/
  `license` from the workspace), [src/lib.rs](src/lib.rs) crate-doc skeleton, and add the crate to the workspace members.

- [x] **M1.2** -- Seed Tier-1 [DESIGN-NOTES.md](DESIGN-NOTES.md), Tier-2 [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md),
  and the Tier-3 session (D-1...D-13); register the crate in the master and crate-local plans. CI covers it
  automatically via the workspace build.

### M2 -- Encoding seam and core types

- [x] **M2.1** -- `WtfEncoding` trait (associated `Unit`, the `NUL` terminator unit) and the `Wtf16`
  encoding (`Unit = u16`) (D-2/D-3).

- [x] **M2.2** -- `WtfString<E>` (owns `Vec<E::Unit>`) and `WtfStr<E>` (`#[repr(transparent)]` over
  `[E::Unit]`): the **always-terminated** storage invariant (hidden trailing `0x0000`; `len()`/spans exclude
  it), construction (`new`, `from_units`), content access on `WtfStr` (`as_units`, `len`, `is_empty`,
  `has_interior_nul`), the `Wtf16String`/`Wtf16Str` aliases, and std parity
  (`Deref`/`AsRef`/`Borrow`/`ToOwned`/`Default`/`Clone`). The FFI pointer surface is M4. (The old M2.2/M2.3
  are merged: the owned type exposes content access *through* `Deref`, so the type definitions and the
  std-parity plumbing cannot compile independently.) (D-4/D-6/D-7)

- [x] **M2.3** -- Tests (sibling [tests.rs](src/string/tests.rs)): the terminator invariant survives construction/clone; spans
  never include the terminator; interior-NUL content is preserved and `has_interior_nul()` reports it;
  `Deref`/`ToOwned` round-trip; empty and large inputs.

### M3 -- str/String conversions and comparison

- [x] **M3.1** -- Boundary conversions: `From<&str>` / `From<String>` (encode once), `into_string() -> Result`
  and `to_string_checked() -> Option` (fallible exact -- `to_string` is avoided since it would collide with
  `ToString` once `Display` lands in M3.2), and `to_string_lossy()` (U+FFFD) (D-8).

- [x] **M3.2** -- `Display` (lossy) and `OsStr`-style escaped `Debug`.

- [x] **M3.3** -- `Ord` / `PartialOrd` / `Eq` / `PartialEq` / `Hash` (binary over units), and cross-type
  comparison with `&str` where it is unambiguous.

- [x] **M3.4** -- Property tests: round-trips through `str`/`String`, ill-formed surrogates preserved in
  storage and replaced by lossy conversion, `> MAX_PATH` lengths, mixed BMP/astral content.

## Moved 2026-08-19 -- M4 FFI surface

### M4 -- FFI surface

- [x] **M4.1** -- Counted access (`as_ptr` + `len`) and the terminated `LPCWSTR` pointer accessor, documented
  as valid only when `has_interior_nul()` is false (D-7/D-10).

- [x] **M4.2** -- Output constructors: `with_capacity` (overflow-guarded), `as_mut_ptr`, and
  `unsafe set_len_from_ffi(content_units)` (explicit content-length convention that appends the terminator,
  never inspecting the buffer) for caller-allocated buffer-fill APIs (D-9).

- [x] **M4.3** -- `unsafe from_wide_ptr(ptr, len)` for callee-allocated buffers, with an explicit safety
  contract (ownership, count semantics, no reference retained; `len == 0` never dereferences `ptr`) (D-9).

- [x] **M4.4** -- Tests: mock buffer-fill (excludes-NUL, includes-NUL via content length, and a trailing
  content-NUL preserved verbatim) rebuilds the invariant; `from_wide_ptr` copies losslessly and is safe at
  `len == 0`; terminated pointer round-trips through a `from_wide_ptr`.

## Moved 2026-08-20 -- M5 Windows OsStr/OsString interop

### M5 -- Windows OsStr/OsString interop

- [x] **M5.1** -- `cfg(windows)` lossless bridge: `from_os_str` / `to_os_string`, `from_wide` / `encode_wide`
  vocabulary aliases, and `From`/`Into` conversions between `OsStr`/`OsString` and `Wtf16Str`/`Wtf16String`.
  Conversion-based both ways (lossless, incl. unpaired surrogates); no borrowing `AsRef<OsStr>` because the
  backing widths differ (D-5/D-8/D-14).

- [x] **M5.2** -- Integration test (Windows): lossless `OsStr` -> `Wtf16String` -> `OsString` round-trips
  including unpaired surrogates and interior NULs (a ~400-case bulk sweep), a real wide `lstrlenW` call fed
  from `as_terminated_ptr()`, and a counted `as_ptr` hand-off matching `encode_wide` -- all with no
  conversion.

  Completing M5 unblocks `crates/windows-file-watcher` -> M8 -> M8.1 (migrate `RelativeName` to
  `Wtf16Str`/`Wtf16String`); see [../windows-file-watcher/CHECKLIST.md](../windows-file-watcher/CHECKLIST.md).
