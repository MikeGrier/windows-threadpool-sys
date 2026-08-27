# Checklist: workspace

Workspace-level and cross-cutting work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

No workspace-level milestones are currently pending. Add future cross-cutting workspace work here as new
milestones.

Three completed groups govern how contracts are written and maintained here, and are worth reading before
touching one:

- [M1 (2026-08-27)](COMPLETED-CHECKLIST.md#moved-2026-08-27-m1) recorded the
  [ten specification-gap categories](DESIGN-NOTES.md#specifying-a-delivery-contract) -- what a contract fails
  to say.
- [M2 (2026-08-27)](COMPLETED-CHECKLIST.md#moved-2026-08-27-m2) addressed
  [restatement drift](DESIGN-NOTES.md#restatement-drift) -- a *correct* rule failing to reach every place
  that states it -- and made the value-level facts derived rather than restated.
- [M3 (2026-08-27)](COMPLETED-CHECKLIST.md#moved-2026-08-27-m3) did the same for the *sequencing* rules,
  through a shared `ContractChecker` the crate's own tests, the harness, and consumers all bind to.

M1's follow-on audits are tracked per crate, not here:
[windows-overlapped-io-sys M14](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[windows-ioring-sys M10](crates/windows-ioring-sys/CHECKLIST.md)
(windows-file-watcher's M14 is complete).
