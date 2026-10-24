# Checklist: fixed-hash-table

What this component is: [README.md](README.md). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md).

## FHT-M1 -- A fixed-capacity map and set

Written by durable-ioring's [DI-23.5.1](../durable-ioring/COMPLETED-CHECKLIST.md#di-2351), from
[DI-D-106](../durable-ioring/DESIGN-NOTES.md#di-d-106).

- [x] **FHT-1.1** -- The map's core: inline or heap storage, linear probing with backward-shift deletion, and an insert refused past capacity. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#fht-11)

- [x] **FHT-1.2** -- The rest of the map's affordances: the entry API, iteration, `retain`, `drain`, `try_extend`, and the standard traits. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#fht-12)

- [x] **FHT-1.3** -- The set, over the map: refusal past capacity, lookup and removal, iteration, and the set operations that need no growth. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#fht-13)

- [ ] **FHT-1.4** -- **README examples compiled as doctests**, through the crate's `#![doc = include_str!("../README.md")]`,
  and a `sabotage.json` for probing, backward-shift deletion, `retain`'s re-examination of a slot after a removal, the set's `Difference`, and the capacity refusal, each
  shown caught.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M23` -> `DI-23.5.2` (declare
  > capacity and refuse past it). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).
