# Checklist: windows-topology-sys

The `MMT-*` plan -- the MachineMemoryTopology reshape that gated PR #56 -- is complete and archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) under `Moved 2026-09-03`, together with the M1-M4
enumeration plan that preceded it. Cite item IDs (`MMT-1.1`, `M4+.1`, `M5+.4`, ...) against that file.

Decisions live in [DESIGN-NOTES.md](DESIGN-NOTES.md), which is the authority for current behaviour;
the archived checklist records what was *done*, not what is *true now*.

## M6: one record walk, per D-24

Opened 2026-09-03 by the PR #56 diff review (`SH-3.1.1`), which found the crate''s two record
decoders internally coherent and mutually opposite. [D-24](DESIGN-NOTES.md#d-24) is the ruling this
milestone implements: **one shared walk, no panic, incoherence recorded in the returned data, and no
trust boundary** -- the OS is trusted for structural validity, and the careful walk is simply how
variable-length records are traversed correctly.

**All five landed in one commit, and the split was wrong.** M6.1 produces anomalies, so it cannot
compile without M6.2''s type; neither can be warning-free until M6.3/M6.4 give them a consumer; and
M6.4 changes `enumerate`''s signature, which is what M6.5 surfaces. They are one coupled change and
are recorded as such rather than teased into a fiction of five commits.

**One measurement changed the design.** The obvious minimum record size for the relationship walk is
`size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>()` -- and it is **wrong**. That struct is 80
bytes because its union is as large as `GROUP_RELATIONSHIP` (72), while a real processor-core record
is 8 + 40 = 48. Using it would have rejected every processor, cache and NUMA record on every machine.
The minimum is the 8-byte header, and each body bounds its own reads instead.

**The amplification is closed by construction and witnessed, not argued.** With a 48-byte record
flush against a `PAGE_NOACCESS` page and `GroupCount = 65535`, the unbounded read raises `0xC0000005`
and the record-bounded one returns cleanly.

- [x] **M6.1** -- **A shared, self-bounding record walk.** New private module: an iterator over a
  `Size`-chained record list, parameterised by the offset of the `Size` field and the minimum record
  size, yielding a **record view bounded by its own `Size`**. The view''s read accessor returns
  nothing when the read would leave the record, so a trailing array cannot be read past the record
  that declares it -- the `GroupCount` amplification closes *by construction*, not by a separate
  check. Built first and unused; `walk.rs` and `cpu_set.rs` adopt it in M6.3/M6.4.

- [x] **M6.2** -- **Vocabulary for a record that did not fit, and somewhere for it to live.** A public
  anomaly type carrying the [`Source`](src/observation.rs) that was being read, the byte offset, and
  what was wrong, plus a new `MachineMemoryTopology` field to carry them. Breaking (the struct has
  public fields and is deliberately hand-constructible, so it does not take `#[non_exhaustive]`),
  which is free on this branch. `serde(default)` so an existing description still deserializes.

- [x] **M6.3** -- **Port `cpu_set::decode` to the shared walk.** Its existing checks become the shared
  ones; its silent `break` becomes a recorded anomaly. Behaviour for well-formed input is unchanged,
  which its five malformed-input tests should confirm without being rewritten.

- [x] **M6.4** -- **Port `walk::decode` to the shared walk, and delete the `assert!`.** This is the
  item with the actual defect in it: a zero `Size` currently panics, `offset + size` is never checked
  against the buffer, and `read_group_affinities` reads `GroupCount` x 16 bytes unbounded. All three
  resolve into the shared walk. Add the malformed-input tests this file has never had.
  Verify the amplification is closed the way `cpu_set`''s was -- a guard-page harness, since the
  decoded output is identical either way and no ordinary test can witness it.

- [x] **M6.5** -- **Surface the anomalies through `discover()`**, and state the policy where a reader
  will meet it: the module docs of both walks, which currently say opposite things about trust.

## Deferred, and why

Two things were deliberately left out of the reshape rather than forgotten:

- **CPU-set flag bit positions** ([D-23](DESIGN-NOTES.md#d-23)). `SYSTEM_CPU_SET_INFORMATION::AllFlags`
  reads constant zero on this build, *even after* `SetProcessDefaultCpuSets` succeeds and
  `GetProcessDefaultCpuSets` confirms the allocation. The bit positions are therefore neither
  confirmed nor falsifiable here; verification needs a machine that populates the byte. This is a
  blocked measurement, not an unwritten one.

- **The planner adapters.** Per [D-21](DESIGN-NOTES.md#d-21) this crate is the refined view of what the
  platform publishes and is self-justified as such; the adapter onto
  [topology-planner](../topology-planner/CHECKLIST.md)'s traits belongs on the planner's side of the
  boundary, and is planned there.
