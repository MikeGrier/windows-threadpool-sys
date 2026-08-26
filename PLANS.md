# Plans

Master tracker for every active checklist in the repository. Each source-component also keeps its own
plans tracker: [crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md),
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md),
[crates/windows-file-watcher/PLANS.md](crates/windows-file-watcher/PLANS.md),
[crates/wtf-string/PLANS.md](crates/wtf-string/PLANS.md),
[crates/windows-ioring-sys/PLANS.md](crates/windows-ioring-sys/PLANS.md), and
[crates/windows-topology-sys/PLANS.md](crates/windows-topology-sys/PLANS.md). Checklists whose work is
finished move to [COMPLETED-PLANS.md](COMPLETED-PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [crates/windows-ioring-sys/CHECKLIST.md](crates/windows-ioring-sys/CHECKLIST.md) | in progress | Memory-safe Rust over the Windows `IoRing` submission/completion ring, as a new crate. M1-M7 (ring lifecycle through the `ring-copy` topology-aligned sample) are complete and archived. Only the parked, pinned-thread `M6+` work remains. | [crates/windows-ioring-sys/DESIGN-NOTES.md](crates/windows-ioring-sys/DESIGN-NOTES.md) |
| [crates/windows-file-watcher/CHECKLIST.md](crates/windows-file-watcher/CHECKLIST.md) -> M13 | in progress | Consumer test surface: an off-by-default `test-util` feature, docs, and an example letting a downstream consumer drive its own notification-handling code with synthetic notifications through the real `Receiver`. The first instantiation of the cross-crate "enable consumers to go below" testability pattern. | [crates/windows-file-watcher/DESIGN-NOTES.md](crates/windows-file-watcher/DESIGN-NOTES.md) |

Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
