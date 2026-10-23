# Checklist: rc-errors

What this component is: [README.md](README.md). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md). This file holds only work in
scope now; parked work is in [PARKED.md](PARKED.md).

## RCE-M1 -- The shared error, the static error, and the report

Written by durable-ioring's [DI-23.4.1](../durable-ioring/COMPLETED-CHECKLIST.md#di-2341), from
[DI-D-105](../durable-ioring/DESIGN-NOTES.md#di-d-105). Every item's tests run on stable; Miri is parked
([RCE-D-8](DESIGN-NOTES.md#rce-d-8)).

- [ ] **RCE-1.1** -- **The allocator interface** ([RCE-D-3](DESIGN-NOTES.md#rce-d-3)): `unsafe trait Allocator` with
  `allocate` and `deallocate`, `AllocError`, and `Global` over `alloc::alloc`, mirroring upstream's names and signatures,
  and usable as `dyn Allocator + Sync`. `Global` returns a dangling, aligned block for a zero-sized layout and frees none,
  and reports failure without panicking. The `NOTICE` file crediting the standard library ([RCE-D-9](DESIGN-NOTES.md#rce-d-9)).

- [ ] **RCE-1.2** -- **The shared error** ([RCE-D-4](DESIGN-NOTES.md#rce-d-4), [RCE-D-5](DESIGN-NOTES.md#rce-d-5)): the
  `repr(C)` block, the fallible constructor and its `Global` variant handing the error back on failure, `Clone`, `Drop`,
  `Deref` to `dyn Error + Send + Sync`, `Display`, `Debug`, `Error`, identity equality, and `Send` and `Sync`. Tested with a
  recording allocator that checks every free's pointer and layout, counts allocations against frees and poisons a freed
  block ([RCE-D-8](DESIGN-NOTES.md#rce-d-8)): the last drop frees once and drops the error once, on another thread too;
  clones dropped concurrently free once; a failing allocator returns the error undropped; a zero-sized and an over-aligned
  error; downcasting; and a source delegated.

- [ ] **RCE-1.3** -- **The static error and the report** ([RCE-D-6](DESIGN-NOTES.md#rce-d-6),
  [RCE-D-7](DESIGN-NOTES.md#rce-d-7)): a `const`-constructible static error holding a message; the two-form report, `Clone`,
  dereferencing to `dyn Error + Send + Sync`, comparing by identity and never equal across forms. The README gains examples
  of both, compiled as doctests through the crate's `#![doc = include_str!("../README.md")]`.

- [ ] **RCE-1.4** -- **Sabotage entries** in a `sabotage.json` for the free condition, the count's increment and the
  layout the free uses, each shown caught; and for the decrement's `Release` and the fence's `Acquire`, recorded as
  surviving ([RCE-D-8](DESIGN-NOTES.md#rce-d-8)).

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M23` -> `DI-23.4.2` (the
  > core's faults carry the report). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).
