# Checklist: workspace

Workspace-level and cross-cutting work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

No workspace-level milestones are currently pending. Add future cross-cutting workspace work here as new
milestones.

Two completed groups govern how contracts are written and maintained here, and are worth reading before
touching one:
[M1 (2026-08-27)](COMPLETED-CHECKLIST.md#moved-2026-08-27-m1) recorded the
[ten specification-gap categories](DESIGN-NOTES.md#specifying-a-delivery-contract) -- what a contract fails to
say -- and [M2 (2026-08-27)](COMPLETED-CHECKLIST.md#moved-2026-08-27-m2) addressed
[restatement drift](DESIGN-NOTES.md#restatement-drift), the separate failure mode where a *correct* rule fails
to reach every place that states it.

M1's follow-on audits are tracked per crate, not here:
[windows-overlapped-io-sys M14](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[windows-ioring-sys M10](crates/windows-ioring-sys/CHECKLIST.md)
(windows-file-watcher's M14 is complete).
