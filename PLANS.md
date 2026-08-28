# Plans

Master tracker for every active checklist in the repository. Each source-component also keeps its own
plans tracker: [crates/windows-file-enumeration-sys/PLANS.md](crates/windows-file-enumeration-sys/PLANS.md),
[crates/windows-file-watcher/PLANS.md](crates/windows-file-watcher/PLANS.md),
[crates/windows-impersonation-token-sys/PLANS.md](crates/windows-impersonation-token-sys/PLANS.md),
[crates/windows-ioring-sys/PLANS.md](crates/windows-ioring-sys/PLANS.md),
[crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md),
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md),
[crates/windows-topology-sys/PLANS.md](crates/windows-topology-sys/PLANS.md), and
[crates/wtf-string/PLANS.md](crates/wtf-string/PLANS.md). Checklists whose work is finished move to
[COMPLETED-PLANS.md](COMPLETED-PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [crates/windows-ioring-sys/CHECKLIST.md](crates/windows-ioring-sys/CHECKLIST.md) | in progress | Memory-safe Rust over the Windows `IoRing` submission/completion ring, as a new crate. M1-M7 (ring lifecycle through the `ring-copy` topology-aligned sample) are complete and archived. Only the parked, pinned-thread `M6+` work remains. | [crates/windows-ioring-sys/DESIGN-NOTES.md](crates/windows-ioring-sys/DESIGN-NOTES.md) |

Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
