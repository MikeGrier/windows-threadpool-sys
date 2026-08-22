# Checklist: windows-file-watcher

Memory-safe Windows path-change watcher. The design session that opened the crate recorded D-1...D-20 in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md).
The authoritative Tier-1 set is [DESIGN-NOTES.md](DESIGN-NOTES.md), which now runs to **D-68** -- later
decisions (D-21 from M1 review, D-22...D-26 and D-34/D-35 from M2, D-36...D-49 from M3, D-50...D-52 from M4,
D-53...D-59 from M5, D-60...D-65 from M6, D-32 from M8.1, D-66...D-68 from M9.1...M9.3, and D-25/D-27...D-31
plus D-33 from the [2026-08-21 fault-protocol session](design-sessions/DESIGN-SESSION-2026-08-21-fault-protocol-and-doorbells.md),
which **overturned D-16**) are added there as milestones complete.

Work items are dependency-ordered. Each milestone ends with integration tests. The implicit
end-of-milestone gate (default **and** `--all-features` build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

> **NEXT ACTIONABLE ITEM: M9.4** -- add session/watch lifecycle operations to the data model and
> generalize the harness to track them by name. M1 through M8 are archived; M9.1 through M9.3 are done,
> M9.5 follows M9.4, and M9+ / M-inf hold only parked, ungated follow-on work.

## M4 -- Coalescing by directory and file targets

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m4-coalescing-by-directory-and-file-targets).

## M5 -- Fault model and the retry protocol

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m5-fault-model-and-the-retry-protocol).

## M6 -- Coarse fallback

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m6-coarse-fallback).

## M7 -- Documentation, examples, stress

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m7-documentation-examples-stress).

## M8 -- Adopt wtf-string for relative names

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m8-adopt-wtf-string-for-relative-names).

## M9 -- Data-driven scenario stress

A load/stress suite whose scenarios are *data*, not one hardcoded test function per behavior: a scenario
is a value (an ordered list of filesystem operations plus timing parameters) that a single shared harness
executes and checks against the same generic invariants (no wedge, no panic, every desync is reported
rather than silently swallowed -- D-12). New scenarios are added by describing them, not by writing new
test bodies. Parameters (entity counts, wait-duration ranges, PRNG seed) are overridable so the same suite
can be run wider without code changes. Re-planned after M9.3: a scenario's *filesystem* actions (M9.1) are
only one dimension: a real client also opens and closes sessions and adds/removes watches while a
directory is live, and that lifecycle churn is a first-class part of the basic tier, not deferred to M9+
(M9+ is specifically the *concurrency* axis -- multiple threads, spoilers, nesting, queue overwhelm -- not
the single-threaded lifecycle churn M9.4/M9.5 add). This milestone covers only the single-modifier basics
the user asked to start with; concurrent modifiers, held-open "spoiler" handles, nested operations, and
queue overwhelm are explicitly deferred to M9+ below once M9 is solid.

- [x] **M9.1** -- Data-driven scenario model: an `Operation` enum (create file, delete path, rename,
  make directory, wait) and a `Scenario { label, operations: Vec<Operation> }` (or builder) that a
  harness can execute mechanically -- no scenario-specific logic outside the data. Wait durations and any
  choice points are drawn from a small seeded deterministic PRNG (no external `rand` dependency needed;
  this crate has none today), defaulting to a fixed seed for reproducibility (per the repo's no-random-
  sampling-without-approval rule) with an env-var override to explore other seeds. Record the seeding
  decision in [DESIGN-NOTES.md](DESIGN-NOTES.md) with a new D-number.

- [x] **M9.2** -- Shared harness: given a `Scenario`, create a temp directory, subscribe a watch, apply
  every `Operation` in order (honoring `Wait`), and assert only the scenario-independent invariants: the
  watch never wedges (a liveness/notification deadline is always met while operations are still being
  applied), no panic, and every `Notification::Desync` is a reported loss rather than silence (D-12). The
  harness takes the scenario and its parameters (counts, timing ranges, seed) as arguments -- it has no
  hardcoded scenario knowledge. **Scaled for hundreds of thousands of operations per run (D-67):** the
  harness reports bounded per-kind tallies (`HarnessOutcome`), never a growing `Vec<Notification>`, and
  drains the queue non-blockingly after every operation so a long run never backs up the crate's own
  bounded queue between checks; `Operation::Repeat` keeps a large scenario's data small regardless of how
  many times it actually runs.

- [x] **M9.3** -- Basic scenario library, expressed as data through M9.1/M9.2: (a) a few files with a
  burst of changes, scaled up with `Operation::Repeat` to the hundreds-of-thousands-of-operations range a
  real stress run is expected to exercise; (b) delete-wait-reintroduce with irregular (PRNG-drawn) wait
  durations; (c) plain renames; (d) a directory created with the name a file used to occupy, and vice versa
  (cross-type name reuse); (e) a fast two-entity swap race: renaming file `x` -> `y` while concurrently
  (within the same operation batch, minimal or zero inter-op wait) renaming directory `z` -> `x`, to probe
  whether the two renames are ever misattributed to each other. **Found and fixed during this item (D-68):**
  applying real syscalls at hundreds-of-thousands scale is itself slow enough (~1,800 ops/sec measured) to
  trip the harness's fixed 120s timeout on throughput alone; `HarnessParams::for_operation_count` scales
  the budget from the scenario's own operation count so only a genuine stall still fails the assertion.

- [ ] **M9.4** -- Session/watch lifecycle operations: extend the data model with operations that open and
  close *sessions* and subscribe and cancel *watches* mid-scenario (`Monitor::session` mints an independent
  channel per call -- D-2 -- so this is not a variation on M9.1's fixed single watch, it is a second kind of
  entity the harness must track by name and drain independently). Generalize the M9.2 harness from one
  fixed session/watch/receiver to a name-keyed table so a scenario can reference "the watch/session named
  X" from a later operation. Same generic invariants apply: no wedge, no panic, every desync counted; a
  session or watch that is already closed when an operation targets it is a scenario-authoring bug (assert),
  not a fault the harness tolerates -- unlike the M9+ .2 "spoiler" case, which is deliberately about a live
  handle blocking an operation.

- [ ] **M9.5** -- Lifecycle scenario library built on M9.4 (sessions/watches opened and closed while
  filesystem churn continues underneath, watches re-subscribed to a path a just-closed watch covered, etc.),
  then wire the complete M9.3+M9.5 scenario set into an opt-in integration test (gated the same way as
  [tests/stress.rs](tests/stress.rs), consistent naming e.g. `WINDOWS_FILE_WATCHER_STRESS`) with
  parameterizable counts/seed. Integration test for the milestone.

## M9+ -- Concurrent modifiers, spoilers, nesting, and queue overwhelm

Gated on M9 (above) being solid: these widen the same data-driven model once the single-modifier basics
pass, per the user's own "start simple ... over time" sequencing. Not started until M9 completes.

- [ ] **M9+.1** -- Concurrent modifiers: parameterize the M9.2 harness to apply a scenario's operations (or
  several independent scenario instances) from multiple threads concurrently, with the modifier count as a
  parameter, still checked against the same no-wedge/no-panic/no-silent-loss invariants.

- [ ] **M9+.2** -- "Spoilers": a modifier that holds a file or directory handle open in a way that blocks a
  rename/delete the scenario is attempting, so the scenario must observe and tolerate the resulting Win32
  failure (or retry) rather than wedging. Parameterize which operations are spoiled and for how long.

- [ ] **M9+.3** -- Nested operations: compose operations (e.g., a rename targeting a path while an
  operation on that same path is still in flight) so the scenario model can express operation nesting, not
  just a flat sequence.

- [ ] **M9+.4** -- Queue overwhelm: parameterize scenario load (entity counts, modifier concurrency, burst
  size) specifically to exceed the crate's documented queue capacity, and assert the crate's documented
  backpressure/loss-reporting behavior holds under deliberate overwhelm rather than a wedge or silent drop.

## M-inf -- Horizon (ungated, post-v1)

Parked, not pending. These are the deferred seams recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> D-19,
an explicit design decision that places them outside the v1 scope. That recorded decision -- not the
absence of a current consumer -- is why each is deferred, which is a legitimate deferral rationale (a
resolved, recorded scope decision), not a scope-boundary excuse. Each graduates to a numbered milestone
when a post-v1 line of work takes one up. None is an open obligation of any current milestone.

- [ ] **M-inf.1** -- `ReadDirectoryChangesExW` extended records (`FILE_NOTIFY_EXTENDED_INFORMATION`): surface the
  richer per-record fields on OS versions that support it, behind capability detection, without disturbing
  the basic `FILE_NOTIFY_INFORMATION` surface (D-18/D-19). **Availability is per-filesystem, not merely
  per-OS-version:** even on a build that exposes the API, some filesystems reject the extended structure --
  e.g. ReFS still does not support it (for no good reason) -- so detection must probe the actual target
  volume and fall back to `FILE_NOTIFY_INFORMATION`, never inferring support from the OS version alone.

- [ ] **M-inf.2** -- Digest-based change *verification*: an optional mode that confirms a reported change by
  hashing content, trading cost for fewer spurious notifications (D-19).

- [ ] **M-inf.3** -- Per-volume capability cache: remember detailed-vs-coarse (and extended-record) support per
  volume so establish/re-establish need not re-probe each time (D-17/D-19).
