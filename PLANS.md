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
| [CHECKLIST.md](CHECKLIST.md) | in progress | M3: make the sequencing rules executable too. M2 covered value-level facts; a shared per-watch ContractChecker in the crate covers ordering, bracket entry states, and terminality -- validating the real watcher's output for the first time and collapsing the harness's hand-written sequencing restatements. | [DESIGN-NOTES.md](DESIGN-NOTES.md#restatement-drift) |
| [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) | not started | M14: finish the contract audit -- categories 1, 2, 6, 8, 9 were not examined -- and sweep `outstanding()` for the advisory-predicate hazard. | [crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) |
| [crates/windows-ioring-sys/CHECKLIST.md](crates/windows-ioring-sys/CHECKLIST.md) | in progress | Memory-safe Rust over the Windows `IoRing` submission/completion ring, as a new crate. M1-M7 (ring lifecycle through the `ring-copy` topology-aligned sample) are complete and archived. The parked, pinned-thread `M6+` work and the new M10 contract audit remain. | [crates/windows-ioring-sys/DESIGN-NOTES.md](crates/windows-ioring-sys/DESIGN-NOTES.md) |

Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
