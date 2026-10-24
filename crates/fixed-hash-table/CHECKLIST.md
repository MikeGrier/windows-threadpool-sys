# Checklist: fixed-hash-table

What this component is: [README.md](README.md). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md).

## FHT-M1 -- A fixed-capacity map and set

Written by durable-ioring's [DI-23.5.1](../durable-ioring/COMPLETED-CHECKLIST.md#di-2351), from
[DI-D-106](../durable-ioring/DESIGN-NOTES.md#di-d-106).

- [ ] **FHT-1.1** -- **The map's core** ([FHT-D-3](DESIGN-NOTES.md#fht-d-3) to [FHT-D-7](DESIGN-NOTES.md#fht-d-7)): the
  storage parameter (inline, and boxed under `alloc`), linear probing with backward-shift deletion, `insert` refusing past
  capacity and handing the entry back, `get`, `get_mut`, `contains_key`, `remove`, `clear`, `len`, `is_empty` and
  `capacity`, with the default seedless hasher and a `with_hasher` constructor. Tested for refusal at exactly capacity, a
  replacing insert at capacity, removal keeping every other key reachable under heavy collision, both storages, and a
  zero-sized and a stateful hasher.

- [ ] **FHT-1.2** -- **The rest of the map's affordances** ([FHT-D-8](DESIGN-NOTES.md#fht-d-8)): the entry API with a fallible
  vacant insert, iteration (shared, mutable, consuming, keys, values), `retain`, `drain`, `try_extend`, `Index`, `Debug`,
  `Clone`, `PartialEq` and `Eq`.

- [ ] **FHT-1.3** -- **The set**, over the map: insert refusing past capacity, `contains`, `remove`, iteration, and the set
  operations that need no growth.

- [ ] **FHT-1.4** -- **README examples compiled as doctests**, through the crate's `#![doc = include_str!("../README.md")]`,
  and a `sabotage.json` for probing, backward-shift deletion and the capacity refusal, each shown caught.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M23` -> `DI-23.5.2` (declare
  > capacity and refuse past it). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).
