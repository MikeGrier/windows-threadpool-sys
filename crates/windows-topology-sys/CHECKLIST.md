# Checklist: windows-topology-sys

The `MMT-*` plan -- the MachineMemoryTopology reshape that gated PR #56 -- is complete and archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) under `Moved 2026-09-03`, together with the M1-M4
enumeration plan that preceded it. Cite item IDs (`MMT-1.1`, `M4+.1`, `M5+.4`, ...) against that file.

Decisions live in [DESIGN-NOTES.md](DESIGN-NOTES.md), which is the authority for current behaviour;
the archived checklist records what was *done*, not what is *true now*.

## M6: the record walks' bounds discipline

Opened 2026-09-03 by the PR #56 diff review (`SH-3.1.1`), which found an out-of-bounds read in
`cpu_set.rs` and, next to it, the same defect class unguarded in `walk.rs`. The `cpu_set.rs` half is
already fixed and shipped in that PR; this milestone is the sibling it exposed.

- [ ] **M6.1** -- **`walk::decode` trusts the kernel's `Size` without bounding it against the
  buffer.** Its loop guard is `while offset < length`, which proves only that **one** byte is in
  range, and it then reads `Relationship` and `Size` -- two 4-byte fields at offsets 0 and 4 -- and
  advances by `size` with **no** `offset + size <= length` check at any point. The backing buffer is
  `vec![0_u64; length.div_ceil(8)]`, i.e. exactly `length` bytes whenever `length % 8 == 0`, so a
  record declaring a `Size` that overruns the buffer walks the loop straight past the allocation.
  **This is the same defect the review found in `cpu_set.rs`**, where the `Type` read at offset 4 sat
  outside the guard that covered it. That one was witnessed deterministically: with the buffer placed
  flush against a `PAGE_NOACCESS` page, the original ordering raised `0xC0000005` and the fixed
  ordering returned cleanly. `walk.rs` was **unchanged on that branch**, so it was left out of the PR
  rather than silently widening a merge -- not because it is less real.
  **Do not stop at the loop guard.** `decode_body` is the larger half: it reads each relationship's
  trailing array using counts taken from the record with no bound against the buffer at all, so
  guarding only the outer loop would give false confidence. Both halves, or neither.
  Verify the same way rather than by reasoning: a guard-page harness that faults before the fix and
  returns after it. A test alone cannot witness this -- the decoded output is identical either way,
  which is exactly why it survived review until someone read the guard against the offsets.

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
