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
| [crates/windows-ioring-sys/CHECKLIST.md](crates/windows-ioring-sys/CHECKLIST.md) | in progress | Memory-safe Rust over the Windows `IoRing` submission/completion ring, as a new crate. M1.1 (crate skeleton) is done; the capability surface is next. M7 adds a topology-aligned sample and is blocked on `windows-topology-sys`. | [crates/windows-ioring-sys/DESIGN-NOTES.md](crates/windows-ioring-sys/DESIGN-NOTES.md) |
| [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md) | in progress | Safe enumeration of Windows processor, cache, and memory topology plus a JSON description that can be discovered or fed in. M1 (safe enumeration), M2 (the open-kinded `Domain`/`Topology` description), and M3 (serde behind a default-off feature, with the schema explicitly not semver-covered) are done; crate documentation is next. | [crates/windows-topology-sys/DESIGN-NOTES.md](crates/windows-topology-sys/DESIGN-NOTES.md) |

Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
