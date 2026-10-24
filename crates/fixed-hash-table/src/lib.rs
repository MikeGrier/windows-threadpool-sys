// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![no_std]
// FHT-D-6: no unsafe code. A slot is an `Option`, so nothing here needs it.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// Storage on the heap, allocated once (FHT-D-2, FHT-D-3).
#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(test)]
extern crate std;

mod hasher;
mod map;
mod set;

pub use hasher::{FixedHasher, FixedState};
pub use map::{
    Drain, Entry, FixedHashMap, Full, InlineMap, IntoIter, Iter, IterMut, Keys, OccupiedEntry,
    Slot, VacantEntry, Values, ValuesMut,
};
#[cfg(feature = "alloc")]
pub use map::{HeapMap, ReserveError};
#[cfg(feature = "alloc")]
pub use set::HeapSet;
pub use set::{
    Difference, FixedHashSet, InlineSet, Intersection, SetDrain, SetIntoIter, SetIter,
    SymmetricDifference, Union,
};
