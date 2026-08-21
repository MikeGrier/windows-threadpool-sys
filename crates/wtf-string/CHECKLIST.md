# Checklist: wtf-string

`OsString`-shaped strings with native `u16` (WTF-16) storage. Design and decisions
(D-1...D-16) are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) (Tier 1),
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) (Tier 2), and
[design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md](design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md)
(Tier 3).

Work items are dependency-ordered. Each milestone ends with tests. The implicit
end-of-milestone gate (default build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M8 -- `windows`-crate `Param<PCWSTR>` interop

- [ ] **M8.1** -- Optional, feature-gated `windows`-crate `Param<PCWSTR>` impl so the high-level `windows`
  crate accepts our type directly, without imposing a hard dependency when the feature is off (D-10).

- [ ] **M8.2** -- Tests (Windows, feature-gated): a `Param<PCWSTR>`-bound call fed from our terminated pointer
  with no conversion.

## M9 -- `no_std` / `alloc`-only support

- [ ] **M9.1** -- `no_std` / `alloc`-only support: gate the `std`-only surface (the `OsStr`/`OsString` interop)
  behind a default `std` feature, and build the portable core (storage, `str`/`String` conversions, FFI
  pointer surface) on `alloc` alone (D-11).

- [ ] **M9.2** -- CI: an `alloc`-only (`--no-default-features`) build+test target so the portable core stays
  verified without `std`.

## M10 -- Documentation, examples, publication readiness

- [ ] **M10.1** -- The [README.md](README.md) and [lib.rs](src/lib.rs) top-level docs: the storage model, the
  conversion-cost contract, the FFI surface, and both encoding widths.

- [ ] **M10.2** -- Runnable example: a wide Win32 round-trip (input via `as_ptr`, output via a buffer-fill
  constructor) showing the zero-conversion path.

- [ ] **M10.3** -- Publication readiness: crate metadata, changelog, and a final review pass over the public
  surface; record the remaining deferred seam (below) as reserved.

## M-inf -- Horizon (ungated, post-v1)

Parked, not pending. The remaining item is placed outside the v1 scope by an explicit, recorded design
decision (D-7 makes the C-string companion optional). That recorded decision -- not the absence of a current
consumer -- is why it is deferred, which is a legitimate deferral rationale (a resolved, recorded scope
decision), not a scope-boundary excuse. It graduates to a numbered milestone when a post-v1 line of work
takes up that decision. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] **M-inf.1** -- A checked no-interior-NUL C-string companion type (an enforced-guarantee analog of the
  terminated pointer) (D-7).
