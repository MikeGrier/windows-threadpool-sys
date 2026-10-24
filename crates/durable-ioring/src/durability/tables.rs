// Copyright (c) 2026 Mike Grier
//! The durability core's tables shared across lineages and keyed by lineage first (DI-D-109): one
//! [`Ordered`] table each, where a `BTreeMap` per lineage was, so the storage is made once for all
//! of them and sized from the operations declared rather than once per lineage.

use core::ops::{Bound, RangeBounds};

use super::Key;
use crate::ordered::{Full, Ordered, Present};

/// How many of a lineage's operations stand at each of its epochs: its writes in flight, or held for
/// their gates. An epoch is present only while its count is above zero.
pub(super) type Counts<E> = Ordered<(Key, E), usize>;

/// A key ordered by lineage, then by epoch, then by anything after.
pub(super) trait ByLineage<E> {
    /// Its lineage.
    fn lineage(&self) -> Key;
    /// Its epoch.
    fn epoch(&self) -> E;
}

impl<E: Copy> ByLineage<E> for (Key, E) {
    fn lineage(&self) -> Key {
        self.0
    }

    fn epoch(&self) -> E {
        self.1
    }
}

impl<E: Copy> ByLineage<E> for (Key, E, u64) {
    fn lineage(&self) -> Key {
        self.0
    }

    fn epoch(&self) -> E {
        self.1
    }
}

/// `lineage`'s entries of `table` whose epochs `epochs` covers, in key order.
pub(super) fn of<'a, K, V, E>(
    table: &'a Ordered<K, V>,
    lineage: Key,
    epochs: impl RangeBounds<E>,
) -> Present<'a, K, V>
where
    K: Ord + ByLineage<E>,
    E: Ord,
{
    let starts_after = |epoch: &E| match epochs.start_bound() {
        Bound::Included(start) => epoch < start,
        Bound::Excluded(start) => epoch <= start,
        Bound::Unbounded => false,
    };
    let ends_at_or_after = |epoch: &E| match epochs.end_bound() {
        Bound::Included(end) => epoch <= end,
        Bound::Excluded(end) => epoch < end,
        Bound::Unbounded => true,
    };
    table.between(
        |key| key.lineage() < lineage || (key.lineage() == lineage && starts_after(&key.epoch())),
        |key| {
            key.lineage() < lineage || (key.lineage() == lineage && ends_at_or_after(&key.epoch()))
        },
    )
}

/// The lowest epoch counted for `lineage`.
pub(super) fn lowest<E: Ord + Copy>(counts: &Counts<E>, lineage: Key) -> Option<E> {
    of(counts, lineage, ..).next().map(|(key, _)| key.1)
}

/// Count one more operation of `lineage` at `epoch`.
///
/// # Errors
///
/// The table's refusal, which inside the bound the operations declared give it is an
/// inconsistency the caller reports.
pub(super) fn count_up<E: Ord + Copy>(
    counts: &mut Counts<E>,
    lineage: Key,
    epoch: E,
) -> Result<(), Full> {
    match counts.get_mut(&(lineage, epoch)) {
        Some(count) => *count += 1,
        None => {
            counts.insert((lineage, epoch), 1)?;
        }
    }
    Ok(())
}

/// Count one fewer operation of `lineage` at `epoch`, removing the epoch when none is left.
/// Whether it was counted.
pub(super) fn count_down<E: Ord + Copy>(counts: &mut Counts<E>, lineage: Key, epoch: E) -> bool {
    let Some(count) = counts.get_mut(&(lineage, epoch)) else {
        return false;
    };
    *count -= 1;
    if *count == 0 {
        counts.remove(&(lineage, epoch));
    }
    true
}
