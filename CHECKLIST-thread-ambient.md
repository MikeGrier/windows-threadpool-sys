# Checklist: windows-thread-ambient-sys and windows-namespace-request-sys

Feature-scoped checklist for the `mikegrier/thread-ambient` branch. It covers two new crates and the
workspace-level changes that introduce them, so it lives at the workspace root -- their lowest common
source-component -- rather than inside either crate. Per the naming convention for feature files, it is
deleted outright once every item is complete, with the content moved to
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

**This file is the whole of this branch's work.** [CHECKLIST.md](CHECKLIST.md) holds the deferred
namespace-facility work (M19-M21, M-inf), which this branch imported but does **not** execute; that import
exists so the design decisions landing here do not reference queued work that is absent from `main`.

Authoritative decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md) and, for the first crate, in
[crates/windows-thread-ambient-sys/DESIGN-NOTES.md](crates/windows-thread-ambient-sys/DESIGN-NOTES.md).

**M22-M29 are complete and archived** in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) under `Moved 2026-09-09 21:28:45 -04:00`. What remains below is
parked no longer: every `M26+` item was gated on the namespace-facility design branch reaching
`main`, and both [crates/windows-namespace-request-sys](crates/windows-namespace-request-sys) and
[crates/windows-thread-ambient-sys](crates/windows-thread-ambient-sys) are on `main` now, so the
gate has lifted and the three items are pending. They keep the `M26+` id until someone graduates
them to a number, which `M34.7` in [CHECKLIST.md](CHECKLIST.md) owns.

## M26+ -- Was gated on the namespace-facility design branch landing; that gate has lifted

- [ ] **M26+.1** -- Reconcile the duplicated design background. This branch imported
  [DESIGN-NOTES.md](DESIGN-NOTES.md)'s namespace-plane section and its design session byte-identical from
  `mikegrier/pseudo-async-file-ops` so the merge would resolve automatically, then corrected part of it
  under M22.1. Once both branches are on `main`, verify that exactly one statement of each fact survived
  the merge and that the corrected statements won, since an automatic resolution of near-identical text is
  precisely how a superseded statement survives unnoticed.

- [ ] **M26+.2** -- Apply the M22.2 narrowing to M21.2 in [CHECKLIST.md](CHECKLIST.md). That item still
  says the error-mode sub-question "needs measurement on both ARM64 and x64 rather than reasoning", which
  M22.2 has since performed: `SEM_NOALIGNMENTFAULTEXCEPT` is rejected by `SetThreadErrorMode` outright, so
  it drops out of the question entirely and needs no architecture pair, and only `SEM_NOGPFAULTERRORBOX`
  remains a genuine policy question. The narrowing was deliberately **not** written into `CHECKLIST.md`
  here, to keep that file byte-identical to the branch it was imported from; the cost of that choice is
  this item, without which `main` would carry a request to measure something already measured. Record the
  measured trap in the same edit: an invalid bit fails the whole `SetThreadErrorMode` call, so a
  forced-plus-transplanted combination installs nothing if the transplanted part is invalid.

- [ ] **M26+.3** -- Make the merge-or-delete decision on the duplicated path preparation. M24.4 copied
  `windows-file-enumeration-sys`'s `path.rs` into `windows-namespace-request-sys` rather than depending on
  it, because that crate is released and this one was not, and this branch exists to reach publication with
  minimal impact on what already ships. **The de-duplication happens after this branch merges with `main`**,
  which is what gates this item -- not a release of the new crate. Decide then: either make the enumeration
  crate consume it and delete the older copy, or keep both and record what makes them genuinely separate. Do **not** let the duplication become permanent by nobody circling back -- that is
  the failure mode the duplicate-then-decide procedure exists to prevent, and it is why this item is here
  rather than only in a design note. Until it is settled, a fix to either copy must be applied to both.
  See [crates/windows-namespace-request-sys/DESIGN-NOTES.md](crates/windows-namespace-request-sys/DESIGN-NOTES.md)
  -> `D-9`.
