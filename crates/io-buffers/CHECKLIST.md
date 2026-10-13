# Checklist: io-buffers

Buffers for asynchronous I/O, portable. See [README.md](README.md) for what the crate is and
[DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## IB-M1 -- The contract, adopted by the ring and the allocator, and an aligned buffer

Sequenced before `durable-ioring`'s [DI-8.2](../durable-ioring/CHECKLIST.md), which needs the buffer
contract in a portable crate. Items are in dependency order.

- [x] **IB-1.1** -- The buffer contract in a portable crate: `IoBuf`, `IoBufMut`, `RegisteredSpan` and the standard-type implementations, copied from `windows-ioring-sys` with their tests. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#ib-11)

- [ ] **IB-1.2** -- **`NumaBuffer`'s buffer impls move to `win-numa-sys`**
  ([IB-D-4](DESIGN-NOTES.md#ib-d-4)): `win-numa-sys` depends on `io-buffers` and implements
  `io_buffers::IoBuf` and `IoBufMut` for `NumaBuffer`, with the safety argument the ring crate's
  `numa_buffer_io.rs` carries. Additive, so it builds beside the ring crate's own implementation of
  its own trait. The allocator crate's README, which says it defines no buffer trait, says what it
  now does instead.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/windows-ioring-sys` ->
  > `IB-1.3`.

- [ ] **IB-1.3** -- **`windows-ioring-sys` takes the contract from `io-buffers`**: its `buf` module and
  `RegisteredSpan` definition are deleted, `IoBuf`, `IoBufMut` and `RegisteredSpan` are re-exported
  at the same paths so no consumer's import changes, and `numa_buffer_io.rs` is deleted, its
  implementations having moved in `IB-1.2`. The ring crate's tests and sabotages stay green, with
  those that pinned the deleted code re-anchored or retired. **Path-only dependency, deliberately**,
  as for the other unreleased crates `durable-ioring` depends on: a version written now would guess
  one release-please has not chosen, and a path-only dependency makes `cargo publish` refuse, so the
  pin cannot be skipped. This crate must be published before the ring crate's next release; the ring
  crate's checklist carries the pin.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` ->
  > `DI-8.2` (the split), which needs this item. See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

- [ ] **IB-1.4** -- **An owned, aligned buffer** ([IB-D-3](DESIGN-NOTES.md#ib-d-3)): an allocation
  whose alignment the caller chooses at construction, zeroed or copied from bytes, implementing
  `IoBuf` and `IoBufMut`, `Send` and `Sync`, with no `unsafe` needed to construct or use it. The
  interface is this item's to settle: how a bad alignment, an overflowing length and an allocation
  failure are reported (`windows-namespace-request-sys`' `AlignedBuffer` panics on a bad alignment,
  which a buffer built by a consumer from a target's declared geometry would rather receive as an
  error), whether it can grow, and what its `Debug` and `Clone` say. Tests include every alignment
  from one to a page, lengths of zero and not a multiple of the alignment, the allocation's address
  checked, and the drop path. Gated on `IB-1.1`; independent of `IB-1.2` and `IB-1.3`.

## IB-M2+ -- Merging the duplicated copies (parked behind IB-M1)

Parked behind the milestone they name, per the workspace's `M{n}+` convention: each is a decision
the engineer makes with the shared crate proven, under duplicate-then-decide, and each is tracked so
the duplication does not become permanent by being forgotten.

- [ ] **IB-2+.1** -- **Whether `windows-overlapped-io-sys` adopts the shared traits**
  ([IB-D-5](DESIGN-NOTES.md#ib-d-5)): re-export them at the same paths, as the ring crate does in
  `IB-1.3`. A type that today implements both crates' traits would then have two implementations of
  one trait, so this is a decision on that cost and not a mechanical step.

- [ ] **IB-2+.2** -- **Whether the other aligned-buffer copies move onto `IB-1.4`'s buffer:**
  `windows-namespace-request-sys`' `AlignedBuffer`, the aligned allocation in
  `windows-overlapped-io-sys`' `fs.rs`, and the ring crate's test-local `Aligned` structs. Gated on
  `IB-1.4`.
