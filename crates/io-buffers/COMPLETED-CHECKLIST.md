# Completed checklist: io-buffers

Append-only record of finished [CHECKLIST.md](CHECKLIST.md) items.

## Moved 2026-10-12 20:46:32 -04:00 -- IB-1.1: the contract, moved from windows-ioring-sys

### <a id="ib-11"></a>IB-1.1 -- The buffer contract in a portable crate: `IoBuf`, `IoBufMut`, `RegisteredSpan` and the standard-type implementations, copied from `windows-ioring-sys` with their tests. *(completed 2026-10-12 20:46:32 -04:00)*

Per [IB-D-2](DESIGN-NOTES.md#ib-d-2). Built by copying the ring crate's `buf.rs` and its tests, and
`RegisteredSpan` from its `batch.rs`, so each file's header names its source and the commit it was
copied at. **The obligations are unchanged; the words that named the holder are not.** The ring
crate's text said the promise binds "for as long as this crate holds the value", and linked its own
`IoRing::try_pop` and milestones; in a crate no ring owns, the holder is "the I/O layer the value was
handed to", and the module documentation, the two `# Safety` sections and the one test comment that
named the ring crate say so. The three bullets of `IoBuf`'s obligation and `IoBufMut`'s additions are
word for word, except that "this crate does not track an initialized prefix" now says "the I/O layer". `RegisteredSpan`'s field documentation no longer cites the ring's
`RegisteredBuffers::get`, and its derives are the ring's: `Clone`, `Copy`, `Debug`.

What the item said that was not so: it expected sabotage entries that pinned the originals to move
with them, and the ring crate has none for `buf.rs`; the ring crate's own record is that mutation
testing found the null-address case there. So this crate has a manifest of its own,
[sabotage.json](sabotage.json), pinning each implementation's address and length separately, and its
control survives as it should. A run of `cargo-mutants` was started and abandoned: it copies the
whole repository, `.scratch/` included, which holds tens of gigabytes of earlier sweep working
copies, and would not have finished usefully.

The README gained a doctest showing a caller-defined buffer. The ring crate is untouched, as the
item required; `IB-1.3` replaces its pair. The commit carries `Release-As: 0.1.0`.

The item as it stood at completion:

- [x] **IB-1.1** -- **The contract, moved from `windows-ioring-sys`**
  ([IB-D-2](DESIGN-NOTES.md#ib-d-2)): `IoBuf`, `IoBufMut` and `RegisteredSpan`, with the
  implementations for `Vec<u8>`, `Box<[u8]>`, `Arc<[u8]>` and the static slices, and their
  documentation and tests, moved by copy so the obligations keep their text. The ring crate is
  untouched by this item: its own pair stays until `IB-1.3` replaces it, so every commit builds.
  Tests are the ring crate's, moved, plus any the move shows to be missing, and run in the
  `portable` CI job beside `wtf-string`. The sabotage manifest entries that pinned the originals
  move with them. The commit carries `Release-As: 0.1.0`, since release-please attributes a commit
  by the paths it touches and `Cargo.toml` says 0.1.0.
