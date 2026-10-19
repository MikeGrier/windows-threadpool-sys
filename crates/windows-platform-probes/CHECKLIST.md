# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M4 -- Carried over from M2: the items M3 gated

These were written under M2 and were blocked on M3, which is **now complete and archived** in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). They are unblocked, and each was RE-SCOPED rather
than merely delayed -- so each item below carries its own re-scoping note, in the item, where
somebody executing the list will actually meet it.

**The IDs keep their M2 numbers deliberately.** [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) is
append-only and its entries are immutable, and two archived entries already cite M2.4 and M2.14 --
so renumbering would leave dangling references in a file that may not be edited to repair them.
Stable IDs cost a mismatch between an item number and its milestone; renumbering would cost
correctness in the archive.

- [x] **M4.1** -- Model the observation-readable `ParseIncomplete` conditions that `blocking_states` currently omits, so a deleted push site in `cross_check` is caught for all of them rather than for the subset. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m41)

- [x] **M4.4** -- Interleave each candidate with a nearby control instead of measuring the control four runs away from it, and re-measure everything that changes. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m44)

- [x] **M4.2** -- Give the measurement probes the controls needed to act on a dispersion finding, so "gather more data along this axis" does not require editing a `const` and rebuilding. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m42)

- [x] **M4.5** -- Emit the dispersion, not just the median. -> [completed 2026-09-15 UTC-07:00](COMPLETED-CHECKLIST.md#m45)

- [x] **M4.3** -- Close the undrained window at the start of the drained regime with a readiness handshake, and re-measure everything that changes. -> [completed 2026-09-16 UTC-04:00](COMPLETED-CHECKLIST.md#m43)

- [x] **M4.6** -- Make the start gate releasable, so a failed thread spawn cannot deadlock the probe. -> [completed 2026-09-16 UTC-04:00](COMPLETED-CHECKLIST.md#m46)

- [x] **M4.7** -- Make the queue-contention report renderer testable, by taking the observation as an argument instead of measuring inside it. -> [completed 2026-09-16 UTC-04:00](COMPLETED-CHECKLIST.md#m47)

- [x] **M2.15** -- Run the probe suite on a second architecture in CI. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m215)

- [x] **M2.17** -- Cross the corpus dimensions instead of varying one at a time. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m217)

- [x] **M4.11** -- Run the queue-contention and peer-index probes in the CI probe job, so each build carries an ARM64 run of both. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m411)

- [x] **M4.8** -- Have the queue-contention report carry the build identity that produced it, and have the capture scripts require it to agree. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m48)

- [x] **M4.9** -- Route a placement probe's refusal-to-measure through the report sink instead of a
  panic, so a host that cannot be pinned is a reported observation rather than a crash.

  **Gap:** `windows-placement-probe`'s pinning helper asserted on `SetThreadGroupAffinity`, and
  `core_affinity::measure()` reaches it through `time_model_on` / `time_model_placed`. **The
  decision to stop was correct and did not change** -- the assert's own message makes the argument,
  that an unpinned thread "would produce a plausible number that answers a different question, and
  nothing in the output would say so." What was wrong is the *mechanism*: a panic bypasses
  `emit_report`, so the refusal landed on stderr while stdout carried a truncated report with no
  row saying why it stopped.

  **Measured, before and after.** With every pin forced to fail, the old binary wrote three lines
  to stdout -- banner, heading, blank -- and exited 101, with the explanation only on stderr. It
  now writes the refusal itself to stdout and exits 0, matching how the same binary already reports
  a topology-discovery failure.

  **Done:** `pin_current_thread` returns `io::Result`; `time_model`, `time_model_on`,
  `time_model_placed`, `median` and `peer_index_cache::measure` propagate it; both probe binaries
  handle it. The assert's message text is kept -- it was the right message, in the wrong channel.
  `PinSignal` still publishes `PIN_FAILED`, now on the early-return path as well as on an unwind,
  so the consumer never waits on a producer that has left.

  **Two things a later review corrected in the first attempt at this item, both the same mistake --
  a claim made about the code rather than from it.**

  `check_group_support` (then `assert_group_support`) tests the *same* predicate as the
  `usize::BITS` branch, over every discovered processor, and runs before any pin. So converting
  only the branch left this one deciding the outcome: the probe still died with banner, heading and
  nothing else. Fixing one site of a rule and leaving its twin is the blast-radius miss this
  repository has a rule against, and the before/after measurement recorded here did not catch it
  because it forced the pin argument rather than the discovered set. Both sites now refuse.

  `peer_index_cache::measure` **cannot** refuse -- it starts only unpinned runs -- so its `# Errors`
  section documented a failure mode no host can produce, and the entry here said "both probe
  binaries render it" as though both could decline. Only `probe-core-affinity` pins. The signature
  stays `Result` because its helpers are fallible in general; the docs now say that instead of the
  opposite.

  **Also corrected:** the refusal text was shared by both causes while being written for one. For
  the mask-width cause it claimed the processor "was reported by this machine's own topology, so
  this is unexpected rather than a limit of the tool" and invited a bug report -- the exact reverse
  of that cause, which *is* a limit of the tool. The explanation is now a parameter, verified in
  both directions.

  **One correction to this item as written.** The blocker recorded when queuing was **wrong** --
  `windows-placement-probe` was absent from release-please configuration at the
  time. It is now a managed binary release under the root
  [DESIGN-NOTES.md](../../DESIGN-NOTES.md#d-probe-releases) -> `D-PB`; `publish = false`
  prohibits crates.io publication, not binary releases. The `x-probe-*` row
  it called for is deferred to `M4.10`, which turned out to be a larger question than these two
  probes.

- [x] **M4.10** -- Decide whether a probe's report owes a machine-readable row, and make the answer uniform. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m410)

## M5 -- Carried over from M2: unblocked hygiene

**Nothing gates these.** They are grouped last by priority, not by dependency -- none of them touches
the report pipeline, so any of them may be pulled forward ahead of M4 at any time. They were
discovered during M2 and parked there under a heading none of them fit. M3, which gated M4 but never
gated these, is complete and archived.

IDs keep their M2 numbers, for the reason given under M4.

- [ ] **M2.13** -- Lint the completed-checklist archive mechanically in CI.

  Three bookkeeping defects reached review on this branch, and all three are decidable by a script: a
  `###` heading concatenated onto the previous line so it did not parse as a heading at all, three
  more headings with no blank line above them, an archived item body left `- [ ]` after being checked
  off, and a pre-existing entry's heading rewritten -- which the file forbids in its own second line.

  Assert, for every `COMPLETED-CHECKLIST.md`: every `###` heading is preceded by a blank line; no
  `- [ ]` remains; and the file has ZERO deleted lines against the merge base. The last one is the
  append-only invariant, and it is the one a human reviewer is least likely to notice.


- [x] **M2.14** -- Make the two authoring rules this branch earned actually bite. -> [completed 2026-09-13](COMPLETED-CHECKLIST.md#m214)

- [x] **M2.14.1** -- Give this crate a sabotage manifest, so "observed to fail" is a recorded artifact rather than a habit. -> [completed 2026-09-13](COMPLETED-CHECKLIST.md#m2141)

- [x] **M2.14.2** -- Add to CONTRACT INTEGRITY rule 1 the one thing this branch learned that it does NOT already say. -> [completed 2026-09-13](COMPLETED-CHECKLIST.md#m2142)

- [x] **M2.16** -- Repair the garbled `Report` doc comment, and drop the two counts that had rotted beside it. -> [completed 2026-09-16 UTC-04:00](COMPLETED-CHECKLIST.md#m216)

## M8 -- Every probe's normal run emits a row of raw data

Applies [Every probe's normal run emits a row of raw data](DESIGN-NOTES.md#d-every-probe-emits-a-row),
decided by `M4.10`. Each row carries what the probe observed, not a verdict; it is not a formal
contract, and a row whose meaning later changes is renamed rather than reused. Where a probe's prose
and row render the same figures, derive both from one list as the cost probes do (`M2.9`). Where a
probe can decline to measure, its item decides what the row says, with topology's
`report_unmeasured` as the example to follow or to depart from on purpose.

Each row item also moves its probe into `EMIT_A_ROW` in
[tests/the_probes_that_emit_a_machine_readable_row.rs](tests/the_probes_that_emit_a_machine_readable_row.rs),
which fails until it does.

- [ ] **M8.1** -- Run `probe-queue-contention` in the row census with a short sampling, replacing
  the exclusion whose reason ("no short mode") `M4.2` made false, so its row is established by
  running it like the rest.

- [ ] **M8.2** -- `probe-queue-contention` emits a row.
- [ ] **M8.3** -- `probe-peer-index-cache` emits a row.
- [ ] **M8.4** -- `probe-core-affinity` emits a row, including when it declines to measure.
- [ ] **M8.5** -- `probe-pool-growth` emits a row.

- [ ] **M8.6** -- `probe-long-path-aware` and `probe-long-path-unaware` emit rows, one item because
  the finding is the difference between them.

- [ ] **M8.7** -- `probe-error-mode` emits a row.
- [ ] **M8.8** -- `probe-handle-state` emits a row.
- [ ] **M8.9** -- `probe-worker-context` emits a row.
- [ ] **M8.10** -- `probe-device-map` emits a row.
- [ ] **M8.11** -- `probe-ioring` emits a row.
- [ ] **M8.12** -- `probe-completion-port` emits a row.
- [ ] **M8.13** -- `probe-cancel-io` emits a row, including for a case its watchdog abandons.

- [ ] **M8.14** -- Make the row census assert that every registered probe emits a row, so a probe
  added later without one fails rather than joining an exception list.
