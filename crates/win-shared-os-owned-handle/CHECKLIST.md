# Checklist: win-shared-os-owned-handle

A shared owner of a Windows handle. See [README.md](README.md) for what the crate is, and
[DESIGN-NOTES.md](DESIGN-NOTES.md) for why it exists.

## M1+ -- Adoption and publication (parked behind this crate's first merge)

- [ ] **SH-1+.1** -- **Adoption by the crates that wrap `Arc<OwnedHandle>` themselves.** The crate
  exists so that they stop. Each adoption is that crate's own checklist item:
  - [x] `windows-ioring-sys`: its own `SharedFile` is removed, and a safe push takes `SharedHandle`
    (its [D-82](../windows-ioring-sys/DESIGN-NOTES.md#d-82)), landed by #119 -- not the alias first
    planned; its `M31.3`, archived in
    [its COMPLETED-CHECKLIST.md](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m313).
  - [ ] `durable-ioring`: its design names `SharedHandle` as the contract's file type (its
    `DI-D-29`), replacing the crate-local `DurableFile` it had first designed; it is built on it
    from `DI-3.1`.

  Done when both have landed. Two further crates wrap a handle the same way and were parked by the
  engineer on 2026-10-07, each in its own checklist and outside this item:
  `windows-namespace-request-sys`' `NR-inf.1` ([its CHECKLIST.md](../windows-namespace-request-sys/CHECKLIST.md)),
  which is not a drop-in replacement, and `windows-impersonation-token-sys`' `IT-inf.1`
  ([its CHECKLIST.md](../windows-impersonation-token-sys/CHECKLIST.md)).

- [x] **SH-1+.2** -- **Publish to crates.io.** Done: 0.1.0, by the publish workflow on the
  `win-shared-os-owned-handle-v0.1.0` tag, 2026-10-07.
