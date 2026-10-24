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

## Moved 2026-10-23 21:23:33 -04:00 -- FHT-1.2: the rest of the map's affordances

### <a id="fht-12"></a>FHT-1.2 -- The rest of the map's affordances: the entry API, iteration, `retain`, `drain`, `try_extend`, and the standard traits. *(completed 2026-10-23 21:23:33 -04:00)*

Per [FHT-D-8](DESIGN-NOTES.md#fht-d-8), which now records how `Clone` and `try_extend` were built. The item as it stood at
completion:

**The rest of the map's affordances**: the entry API with a fallible vacant insert, iteration (shared, mutable, consuming,
keys, values), `retain`, `drain`, `try_extend`, `Index`, `Debug`, `Clone`, `PartialEq` and `Eq`.

What was done. [entry.rs](src/map/entry.rs) holds `Entry`, `OccupiedEntry` and `VacantEntry`; every inserting path returns
`Full` when the map is full, and `or_insert_with` hands its closure back uncalled rather than make a value it cannot place.
[iter.rs](src/map/iter.rs) holds the iterators, all exact-size and fused, `drain` (which empties the map even when dropped
part-way), and `retain`, which walks from an empty slot so no probe run is split by the start, and re-examines a slot after a
removal shifts an entry into it. [traits.rs](src/map/traits.rs) holds `Debug`, `PartialEq` across storages and hashers,
`Eq`, `Index`, `Clone` for inline maps, `try_clone` for heap maps, and `try_extend`. The read-only lookups moved to an `impl`
needing only `AsRef`, so equality can compare against any storage. A sabotage of `retain`'s re-examination (stepping past the
slot after a removal) failed the wrapping-run test and was reverted; the manifest entry is FHT-1.4's.

## Moved 2026-10-23 21:28:45 -04:00 -- FHT-1.3: the set

### <a id="fht-13"></a>FHT-1.3 -- The set, over the map: refusal past capacity, lookup and removal, iteration, and the set operations that need no growth. *(completed 2026-10-23 21:28:45 -04:00)*

Per [FHT-D-8](DESIGN-NOTES.md#fht-d-8), which now records how the set was built. The item as it stood at completion:

**The set**, over the map: insert refusing past capacity, `contains`, `remove`, iteration, and the set operations that need
no growth.

What was done. [set.rs](src/set.rs) holds `FixedHashSet`, a map of the values to `()`, with `InlineSet` and `HeapSet`;
`insert`, `replace`, `contains`, `get`, `remove`, `take`, `retain` and `clear`. [set/iter.rs](src/set/iter.rs) holds the
iterators and the set operations: `difference`, `intersection`, `symmetric_difference` and `union` as lazy iterators, and
`is_disjoint`, `is_subset` and `is_superset`, each over a second set of any storage and hasher.
[set/traits.rs](src/set/traits.rs) holds `Debug`, `PartialEq` and `Eq`, `Clone` for inline sets, `try_clone` for heap sets,
and `try_extend`. The map's `clear` moved to an `impl` needing only mutable storage, since it hashes nothing; the set could
not call it otherwise. A sabotage of `Difference` (dropping its negation) failed two tests and was reverted; the manifest
entries are FHT-1.4's.
