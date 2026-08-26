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
| [crates/windows-file-watcher-example-test-harness/CHECKLIST.md](crates/windows-file-watcher-example-test-harness/CHECKLIST.md) | in progress | A published EXAMPLE test harness for file-change-notification handlers, built on windows-file-watcher's `test-util` seam: a handler trait, a harness-owned serde schedule format, a driver, a contract-legal seeded generator, oracles, JSON record/replay, and `capture`/`replay` bins. Exposition of the "what you can build on the seam" technique. | [crates/windows-file-watcher-example-test-harness/DESIGN-NOTES.md](crates/windows-file-watcher-example-test-harness/DESIGN-NOTES.md) |

Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
