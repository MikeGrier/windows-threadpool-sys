# Plans

Master tracker for every active checklist in the repository. Each source-component also keeps its own
plans tracker: [crates/windows-file-enumeration-sys/PLANS.md](crates/windows-file-enumeration-sys/PLANS.md),
[crates/windows-file-watcher/PLANS.md](crates/windows-file-watcher/PLANS.md),
[crates/windows-impersonation-token-sys/PLANS.md](crates/windows-impersonation-token-sys/PLANS.md),
[crates/windows-ioring-sys/PLANS.md](crates/windows-ioring-sys/PLANS.md),
[crates/windows-overlapped-io-sys/PLANS.md](crates/windows-overlapped-io-sys/PLANS.md),
[crates/windows-namespace-request-sys/PLANS.md](crates/windows-namespace-request-sys/PLANS.md),
[crates/windows-platform-probes/PLANS.md](crates/windows-platform-probes/PLANS.md),
[crates/windows-thread-ambient-sys/PLANS.md](crates/windows-thread-ambient-sys/PLANS.md),
[crates/windows-threadpool-sys/PLANS.md](crates/windows-threadpool-sys/PLANS.md),
[crates/windows-topology-sys/PLANS.md](crates/windows-topology-sys/PLANS.md), and
[crates/wtf-string/PLANS.md](crates/wtf-string/PLANS.md). Checklists whose work is finished move to
[COMPLETED-PLANS.md](COMPLETED-PLANS.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | not started | M19: propagate the 2026-08-27 platform measurements (IoRing registration replaces the table; the completion-port/`IoRing` fork; `runs_long` as the growth mechanism; the measured 512 default maximum) into the crates whose code or documentation currently assumes otherwise. M20: decide the session-independent path form, now that path resolution is measured to follow the impersonated token's logon session. M21: reconcile with the impersonation and enumeration crates that landed during the session. | [DESIGN-NOTES.md](DESIGN-NOTES.md#remoting-synchronous-namespace-operations) |
| [CHECKLIST-thread-ambient.md](CHECKLIST-thread-ambient.md) | in progress | M22-M23: extract the captured-context composite into `windows-thread-ambient-sys`, a standalone platform layer that captures a thread's ambient state and applies it on another thread. M24-M26: `windows-namespace-request-sys`, marshalable Win32 namespace call parameter sets, over a round-one entry list audited from three real consumers (this repository's watcher and enumeration crates, and `MikeGrier/Globazog-rs`) rather than guessed. M27: `windows-platform-probes`, a durable home for the measurements this workspace's designs rest on, under a three-tier scheme (asserted / ignored / binary-only) where every tier is compiled by an ordinary build. Feature-scoped and deleted when complete; it is the whole of the `mikegrier/thread-ambient` branch's work, and is deliberately separate from the deferred namespace-facility items in [CHECKLIST.md](CHECKLIST.md). | [crates/windows-thread-ambient-sys/DESIGN-NOTES.md](crates/windows-thread-ambient-sys/DESIGN-NOTES.md) |
| [crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) | not started | M14: finish the contract audit -- categories 1, 2, 6, 8, 9 were not examined -- and sweep `outstanding()` for the advisory-predicate hazard. | [crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) |
| [crates/windows-ioring-sys/CHECKLIST.md](crates/windows-ioring-sys/CHECKLIST.md) | in progress | Memory-safe Rust over the Windows `IoRing` submission/completion ring, as a new crate. M1-M7 (ring lifecycle through the `ring-copy` topology-aligned sample) are complete and archived. The parked, pinned-thread `M6+` work and the new M10 contract audit remain. | [crates/windows-ioring-sys/DESIGN-NOTES.md](crates/windows-ioring-sys/DESIGN-NOTES.md) |
| [CHECKLIST-mutation-survivors.md](CHECKLIST-mutation-survivors.md) | not started | Work queued from the workspace-wide cargo-mutants sweep of 2026-09-02, whose findings are kept in [mutation-sweeps/2026-09-02/](mutation-sweeps/2026-09-02/README.md) rather than re-derived -- the run took roughly fourteen hours. 2,792 caught, 1,112 survived, 198 timed out. **The headline numbers mislead in three ways and the README says how**: a timeout in a blocking-API crate is usually a detection that lost its name rather than a gap (measured: one of `windows-waitable-queues`' 120 timeouts fails four tests in 0.00s when re-injected alone), a low score on an executable probe crate is measuring the wrong thing, and three kinds of survivor -- equivalent mutants, unreachable code, and constants that want a `const` assertion -- are not missing tests at all. M1 covers the shipping crates; M2 holds the two crates that are not libraries and whose scope is an engineer's decision; M3 re-runs and prunes rather than hand-editing the tool's output into a second source of truth. | [mutation-sweeps/2026-09-02/README.md](mutation-sweeps/2026-09-02/README.md) |

Add a row here when new work is planned, against [CHECKLIST.md](CHECKLIST.md) or any crate's.
