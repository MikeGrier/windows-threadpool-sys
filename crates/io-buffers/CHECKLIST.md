# Checklist: io-buffers

Buffers for asynchronous I/O, portable. See [README.md](README.md) for what the crate is and
[DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions. IB-M1, the contract and the aligned buffer,
is complete and archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ib-m1-complete).

## IB-M2+ -- Merging the duplicated copies (parked behind IB-M1)

Parked behind the milestone they name, per the workspace's `M{n}+` convention: each is a decision
the engineer makes with the shared crate proven, under duplicate-then-decide, and each is tracked so
the duplication does not become permanent by being forgotten.

- [x] **IB-2+.1** -- `windows-overlapped-io-sys` adopts the shared traits and re-exports them at the same paths. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#ib-2+1)

- [ ] **IB-2+.2** -- **Whether the other aligned-buffer copies move onto `IB-1.4`'s buffer:**
  `windows-namespace-request-sys`' `AlignedBuffer`, the aligned allocation in
  `windows-overlapped-io-sys`' `fs.rs`, and the ring crate's test-local `Aligned` structs. Its
  interface is [IB-D-6](DESIGN-NOTES.md#ib-d-6): fallible to build, where `AlignedBuffer` panics,
  and with no `Clone`, so moving a copy onto it changes its callers' error handling.
