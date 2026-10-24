# Completed checklist: fixed-hash-table

Completed items from [CHECKLIST.md](CHECKLIST.md), append-only.

## Moved 2026-10-23 21:17:06 -04:00 -- FHT-1.1: the map's core

### <a id="fht-11"></a>FHT-1.1 -- The map's core: inline or heap storage, linear probing with backward-shift deletion, and an insert refused past capacity. *(completed 2026-10-23 21:17:06 -04:00)*

Per [FHT-D-3](DESIGN-NOTES.md#fht-d-3) to [FHT-D-7](DESIGN-NOTES.md#fht-d-7). The item as it stood at completion:

**The map's core**: the storage parameter (inline, and boxed under `alloc`), linear probing with backward-shift deletion,
`insert` refusing past capacity and handing the entry back, `get`, `get_mut`, `contains_key`, `remove`, `clear`, `len`,
`is_empty` and `capacity`, with the default seedless hasher and a `with_hasher` constructor. Tested for refusal at exactly
capacity, a replacing insert at capacity, removal keeping every other key reachable under heavy collision, both storages, and
a zero-sized and a stateful hasher.

What was done. [map.rs](src/map.rs) holds `FixedHashMap`, generic over any storage that is a slice of slots through `AsRef`
and `AsMut`, with `InlineMap` (an array, whose slot count a `const` assertion holds to a power of two) and `HeapMap` (a `Vec`
filled once with `try_reserve_exact`, not a boxed slice, which FHT-D-3 now records); `Full` and `ReserveError`;
`get_key_value` and `remove_entry` beside the item's list. [hasher.rs](src/hasher.rs) holds `FixedState` and its hasher. The
tests add to the item's list a 10,000-step fixed-seed run against `std`'s map, a run wrapping past the last slot, and a
table emptied by fifty rounds of churn left with no occupied slot; the slot-count assertion is a `compile_fail` doctest
pinned to its error code. A sabotage of the backward shift (`>=` to `>`) failed four tests and was reverted; the manifest
entries are FHT-1.4's.
