# fixed-hash-table

Hash tables whose capacity is fixed when they are made, and which **refuse an insert when full** rather than grow.

`std`'s `HashMap` is already open-addressed, but it reallocates when an insert outgrows it, exists only with `std`, and
seeds its default hasher from the operating system. Code that allocates everything it needs up front, and must not allocate
or panic for want of memory afterwards, needs a table that says "full" instead. This crate's tables do: an insert past
capacity hands the key and value back.

They are `no_std` and allocation-free by default, with their storage inline. A client that has an allocator can put the
storage on the heap, allocated once, when the table is made, and fallibly. The hasher is a type parameter, as
`core::hash::BuildHasher` makes it: a zero-sized one costs no storage, and one with state can differ per table.

## A map

An inline map's slot count is a const parameter, a power of two; it holds up to seven-eighths of them. Being full is an
answer, and the entry refused comes back:

```rust
use fixed_hash_table::{Full, InlineMap};

let mut ages: InlineMap<&str, u32, 4> = InlineMap::new();
assert_eq!(ages.capacity(), 3);
ages.insert("ada", 36).expect("room");
ages.insert("alan", 41).expect("room");
ages.insert("grace", 85).expect("room");
assert_eq!(ages.insert("edsger", 72), Err(Full { key: "edsger", value: 72 }));

// Replacing a present key's value needs no room.
assert_eq!(ages.insert("ada", 37), Ok(Some(36)));
assert_eq!(ages["ada"], 37);
```

The entry API is the same shape as `std`'s, except that inserting can be refused:

```rust
use fixed_hash_table::InlineMap;

let mut counts: InlineMap<char, u32, 8> = InlineMap::new();
for letter in "abracadabra".chars() {
    *counts.entry(letter).or_insert(0).expect("five letters fit") += 1;
}
assert_eq!((counts[&'a'], counts[&'b'], counts[&'r']), (5, 2, 2));
```

## A set

A set is the map with no values, and has the set operations that make no new set:

```rust
use fixed_hash_table::InlineSet;

let mut evens: InlineSet<u32, 8> = InlineSet::new();
let mut small: InlineSet<u32, 8> = InlineSet::new();
evens.try_extend([2, 4, 6]).expect("room");
small.try_extend([1, 2, 3]).expect("room");

let mut both: Vec<u32> = evens.intersection(&small).copied().collect();
both.sort();
assert_eq!(both, [2]);
assert!(!evens.is_disjoint(&small));
assert_eq!(evens.union(&small).count(), 5);
```

## Storage on the heap

With the `alloc` feature, a table's slots can be allocated when it is made, at a capacity chosen then. Making it is fallible;
nothing after that allocates:

```rust
# #[cfg(feature = "alloc")]
# {
use fixed_hash_table::HeapMap;

let mut map: HeapMap<u64, u64> = HeapMap::try_with_capacity(1000).expect("allocated");
assert!(map.capacity() >= 1000);
for key in 0..1000 {
    map.insert(key, key * key).expect("room");
}
assert_eq!(map.get(&30), Some(&900));
# }
```

## The hasher

The default hasher is seedless, so keys an adversary chooses can be made to collide. Supply a keyed one for those; with
`std`, that can be `std`'s own:

```rust
# #[cfg(feature = "std")]
# {
use fixed_hash_table::InlineMap;
use std::hash::RandomState;

let mut map: InlineMap<String, u32, 16, RandomState> = InlineMap::with_hasher(RandomState::new());
map.insert(String::from("key"), 1).expect("room");
assert_eq!(map.get("key"), Some(&1));
# }
```

The design is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md), and the work in [CHECKLIST.md](CHECKLIST.md).
