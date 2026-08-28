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
| [CHECKLIST.md](CHECKLIST.md) | not started | M19: propagate the 2026-08-27 platform measurements (IoRing registration replaces the table; the completion-port/`IoRing` fork; `runs_long` as the growth mechanism; the measured 512 default maximum) into the crates whose code or documentation currently assumes otherwise. M20: decide the session-independent path form, now that path resolution is measured to follow the impersonated token's logon session. M21: reconcile with the impersonation and enumeration crates that landed during the session. | [DESIGN-NOTES.md](DESIGN-NOTES.md#remoting-synchronous-namespace-operations) |
| [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) | not started | M14: finish the contract audit -- categories 1, 2, 6, 8, 9 were not examined -- and sweep `outstanding()` for the advisory-predicate hazard. | [crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) |
| [crates/windows-ioring-sys/CHECKLIST.md](crates/windows-ioring-sys/CHECKLIST.md) | in progress | Memory-safe Rust over the Windows `IoRing` submission/completion ring, as a new crate. M1-M7 (ring lifecycle through the `ring-copy` topology-aligned sample) are complete and archived. The parked, pinned-thread `M6+` work and the new M10 contract audit remain. | [crates/windows-ioring-sys/DESIGN-NOTES.md](crates/windows-ioring-sys/DESIGN-NOTES.md) |

Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
