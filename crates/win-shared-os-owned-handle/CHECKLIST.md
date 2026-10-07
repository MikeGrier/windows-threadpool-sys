# Checklist: win-shared-os-owned-handle

A shared owner of a Windows handle. See [README.md](README.md) for what the crate is, and
[DESIGN-NOTES.md](DESIGN-NOTES.md) for why it exists.

## M1+ -- Adoption and publication (parked behind this crate's first merge)

- [ ] **SH-1+.1** -- **Adoption by the crates that wrap `Arc<OwnedHandle>` themselves.** The crate
  exists so that they stop. Each adoption is that crate's own checklist item:
  - `windows-ioring-sys`: `SharedFile` becomes an alias of `SharedHandle` -- its `M31.3`, queued in
    [its CHECKLIST.md](../windows-ioring-sys/CHECKLIST.md) on 2026-10-07.
  - `durable-ioring`: its design names `SharedHandle` as the contract's file type (its `DI-D-29`),
    replacing the crate-local `DurableFile` it had first designed; it is built on it from `DI-3.1`.

  Done when both have landed.

- [x] **SH-1+.2** -- **Publish to crates.io.** Done: 0.1.0, by the publish workflow on the
  `win-shared-os-owned-handle-v0.1.0` tag, 2026-10-07.
