# Checklist: wtf-string

`OsString`-shaped strings with native `u16` (WTF-16) storage. Design and decisions
(D-1...D-13) are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) (Tier 1),
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) (Tier 2), and
[design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md](design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md)
(Tier 3).

Work items are dependency-ordered. Each milestone ends with tests. The implicit
end-of-milestone gate (default build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M5 -- Windows OsStr/OsString interop

- [ ] **M5.1** -- `cfg(windows)` lossless bridge: `from_os_str` / `to_os_string` (and `AsRef<OsStr>` where it
  fits) via `encode_wide`/`from_wide`; `from_wide` / `encode_wide`-style helpers that borrow our slice with
  zero copy (D-5/D-8).

- [ ] **M5.2** -- Integration test (Windows): lossless `OsStr` -> `Wtf16Str` -> `OsStr` round-trip including
  unpaired surrogates; a real wide Win32 call fed directly from `as_ptr()` with no conversion.

  > **-> CROSS-COMPONENT HANDOFF:** completing M5 unblocks component `crates/windows-file-watcher` -> M8 ->
  > M8.1 (migrate `RelativeName` to `Wtf16Str`/`Wtf16String`). See
  > [../windows-file-watcher/CHECKLIST.md](../windows-file-watcher/CHECKLIST.md).

## M6 -- Documentation, examples, publication readiness

- [ ] **M6.1** -- The [README.md](README.md) and [lib.rs](src/lib.rs) top-level docs: the storage model, the
  conversion-cost contract, and the FFI surface.

- [ ] **M6.2** -- Runnable example: a wide Win32 round-trip (input via `as_ptr`, output via a buffer-fill
  constructor) showing the zero-conversion path.

- [ ] **M6.3** -- Publication readiness: crate metadata, changelog, and a final review pass over the public
  surface; record the deferred seams (below) as reserved.

## M-inf -- Horizon (ungated, post-v1)

Parked, not pending. Each item is **explicitly out of the v1 scope** fixed by its cited design decision
(D-3 defers the `Wtf8` arm; D-7 makes the C-string companion optional; D-10 makes the `windows`-crate
`Param<PCWSTR>` interop optional; D-11 defers `no_std`), so v1 is complete without them **by design** -- the
"blocker" for each is that deliberate scope boundary, not an oversight. Each graduates to a numbered
milestone only if a post-v1 line of work pursues it. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] **M-inf.1** -- The `Wtf8` encoding arm (`WtfString<Wtf8>` / `WtfStr<Wtf8>`): implement the crate-owned
  `WtfEncoding` contract for `u8`/WTF-8 units -- `Unit = u8`; storage is arbitrary WTF-8 (ill-formed-tolerant,
  matching `OsStr`); `encode_str` is the identity on a UTF-8 `str`'s bytes; exact decode is
  valid-UTF-8-or-`None`; lossy decode replaces with U+FFFD; comparison/hash are binary over bytes; `debug_fmt`
  escapes ill-formed sequences losslessly. Storage is a crate-owned `Vec<u8>` whose WTF-8 layout matches
  `OsString`'s but is not built on it. Gives a uniform API across storage widths (D-3).

- [ ] **M-inf.2** -- A checked no-interior-NUL C-string companion type (an enforced-guarantee analog of the
  terminated pointer) (D-7).

- [ ] **M-inf.3** -- Optional, feature-gated `windows`-crate `Param<PCWSTR>` impl so the high-level crate accepts
  our type without a hard dependency (D-10).

- [ ] **M-inf.4** -- `no_std` / `alloc`-only support (D-11).
