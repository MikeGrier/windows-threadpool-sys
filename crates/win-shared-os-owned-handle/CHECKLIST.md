# Checklist: win-shared-os-owned-handle

A shared owner of a Windows handle. See [README.md](README.md) for what the crate is, and
[DESIGN-NOTES.md](DESIGN-NOTES.md) for why it exists.

## M1+ -- Adoption and publication (parked behind this crate's first merge)

- [ ] **SH-1+.1** -- **Adoption by the crates that wrap `Arc<OwnedHandle>` themselves.** The crate
  exists so that they stop: `windows-ioring-sys`' `SharedFile` becomes this type (or an alias of it,
  to stay source-compatible), and `durable-ioring`'s `DurableFile`, still on its own branch, uses it
  directly. Each adoption is that crate's own checklist item, queued there when this crate is on
  `main`.

- [ ] **SH-1+.2** -- **Publish to crates.io.** A published crate cannot depend on an unpublished one,
  so this must precede any published crate's adoption (SH-1+.1).
