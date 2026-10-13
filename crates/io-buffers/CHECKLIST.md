# Checklist: io-buffers

Buffers for asynchronous I/O, portable. See [README.md](README.md) for what the crate is and
[DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions. IB-M1, the contract and the aligned buffer,
is complete and archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ib-m1-complete).

## IB-M2+ -- Merging the duplicated copies (parked behind IB-M1)

Parked behind the milestone they name, per the workspace's `M{n}+` convention: each is a decision
the engineer makes with the shared crate proven, under duplicate-then-decide, and each is tracked so
the duplication does not become permanent by being forgotten.

`IB-2+.1`, the traits, is done and archived. What remains is the aligned-buffer copies, investigated
2026-10-12: three of them, with three different costs, so three items. A fourth family is **not** a
copy of the allocation: the `Vec<u32>` and `Vec<u64>` idiom for four- and eight-byte alignment in
`windows-file-watcher`, `windows-topology-sys`, `windows-file-enumeration-sys` and
`windows-platform-probes` gets its alignment from the element type with no allocation code of its
own, and moving it would add a fallible constructor for no alignment it lacks. It is not scheduled,
and nothing here forecloses it. The historical spikes under the ring crate's `design-sessions/` are
frozen records and are left as they are.

- [x] **IB-2+.1** -- `windows-overlapped-io-sys` adopts the shared traits and re-exports them at the same paths. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#ib-2+1)

- [x] **IB-2+.2** -- `windows-overlapped-io-sys`' `PageBuffers` holds an `AlignedBuf`. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#ib-2+2)

- [ ] **IB-2+.3** -- **The ring crate's three test-local `Aligned` buffers become `AlignedBuf`.**
  `handover.rs`, `flush_barrier.rs` and `flush_barrier_stress.rs` each over-allocate a `Vec<u8>` and
  offset into it, with an `unsafe impl` of the traits and the reason in a comment: a hand-aligned
  allocation cannot be freed under the right layout, which `AlignedBuf` does by allocating with the
  layout. Test-only, and removes three `unsafe impl`s. Two need a fill byte, which `AlignedBuf`'s
  `DerefMut` gives. `handover.rs` and `flush_barrier.rs` run by default; the stress file is
  `#[ignore]`d and has to be run explicitly to be checked.

- [ ] **IB-2+.4** -- **Whether `windows-namespace-request-sys`' `AlignedBuffer` moves onto
  `AlignedBuf`, and by which route.** The engineer's decision, because the type is public API of a
  published crate (0.2.3): it appears in public return types (`Outcome<AlignedBuffer>`, the query and
  security captures) and is re-exported. Its differences from `AlignedBuf` are
  [IB-D-6](DESIGN-NOTES.md#ib-d-6)'s: constructors that panic on a bad alignment and abort on
  allocation failure where `AlignedBuf` returns an error, and a `Clone` where `AlignedBuf` has
  `try_clone`. Every call site passes a constant alignment, and none needs `IoBuf`, since the Win32
  queries are synchronous, so the only gain is one copy of the unsafe allocation code, at the cost of
  a dependency on this crate. Routes: **(a)** replace the type, which is breaking and changes every
  signature that names it; **(b)** keep `AlignedBuffer` as a thin wrapper over `AlignedBuf` with the
  same panicking constructors, `Clone` and equality, which is not breaking and removes the duplicate
  allocation code but leaves two public types; **(c)** leave it. No work until a route is chosen.
