# Checklist: rc-errors

What this component is: [README.md](README.md). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md). This file holds only work in
scope now; parked work is in [PARKED.md](PARKED.md).

## RCE-M1 -- The shared error, the static error, and the report

Written by durable-ioring's [DI-23.4.1](../durable-ioring/COMPLETED-CHECKLIST.md#di-2341), from
[DI-D-105](../durable-ioring/DESIGN-NOTES.md#di-d-105). Every item's tests run on stable; Miri is parked
([RCE-D-8](DESIGN-NOTES.md#rce-d-8)).

- [x] **RCE-1.1** -- The allocator interface: Allocator, AllocError and Global, mirroring upstream, with the NOTICE crediting the standard library. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#rce-11)

- [x] **RCE-1.2** -- The shared error: one repr(C) block, a fallible constructor handing the error back, and counting freed through the recorded allocator, tested with a recording allocator. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#rce-12)

- [ ] **RCE-1.3** -- **The static error and the report** ([RCE-D-6](DESIGN-NOTES.md#rce-d-6),
  [RCE-D-7](DESIGN-NOTES.md#rce-d-7)): a `const`-constructible static error holding a message; the two-form report, `Clone`,
  dereferencing to `dyn Error + Send + Sync`, comparing by identity and never equal across forms. The README gains examples
  of both, compiled as doctests through the crate's `#![doc = include_str!("../README.md")]`.

- [ ] **RCE-1.4** -- **Sabotage entries** in a `sabotage.json` for the free condition, the count's increment and the
  layout the free uses, each shown caught; and for the decrement's `Release` and the fence's `Acquire`, recorded as
  surviving ([RCE-D-8](DESIGN-NOTES.md#rce-d-8)).

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M23` -> `DI-23.4.2` (the
  > core's faults carry the report). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).
