// Copyright (c) 2026 Mike Grier
//! The epochs abandoned above their lineage's mark (DI-D-35, DI-D-109): kept in a table made at
//! construction, each pinning the place of one of its writes, and forgotten once the mark passes
//! it.
//!
//! An abandoned epoch is remembered so a write into it is refused, and so a gate on it is refused
//! or its held operations end as never issued, while the mark has not passed it. Once the mark
//! passes it, the mark answers for it -- durable through means durable or abandoned -- and a write
//! into it is refused as sealed, so nothing needs it; the consumer learned which from the
//! `Abandoned` entry.
//!
//! **The bound.** An epoch is abandoned by abandoning a failure that suspects one of its writes,
//! and the failure holds that write's place until its `Abandoned` entry is popped (DI-D-100). The
//! epoch takes over that place, already held, so recording it never needs a place of its own; and
//! a write is of one epoch, so no two epochs pin one write. The epochs kept are therefore at most
//! the places, at most the operations declared, whatever other failure still holds the same
//! writes: a place is one however many hold it. The table is made for `operations`, and one past
//! it is an inconsistency.

use super::kept::Kept;
use super::suspects::suspects;
use super::window::{Holder, Window};
use super::{Durability, Failure, Key, LineageState, Write};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::domain_lists::DomainLists;
use crate::ordered::Ordered;

/// The abandoned epochs above their lineage's mark, by lineage and epoch, each with the write whose
/// place it pins.
pub(super) type Abandoned<E> = Ordered<(Key, E), u64>;

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// Whether `lineage`'s `epoch` is abandoned and above the mark: an abandoned failure contained
    /// it, so no write may join it (DI-D-35). Below the mark it is forgotten (DI-D-109).
    pub(crate) fn is_abandoned(&self, lineage: Key, epoch: E) -> bool {
        self.lineages
            .get(&lineage)
            .is_some_and(|state| !state.ended)
            && self.abandoned.get(&(lineage, epoch)).is_some()
    }

    /// `failure`, out of the table, was abandoned: every epoch of a live lineage it suspects a
    /// write of, above the mark, is kept, pinning the place of the first such write.
    pub(super) fn abandon_epochs(&mut self, failure: &Failure<E, I>) {
        let lost = abandon(
            &self.lists,
            &self.writes,
            &self.kept,
            &self.lineages,
            failure,
            &mut self.abandoned,
            &mut self.window,
        );
        if let Some((what, op)) = lost {
            self.inconsistent(what, format_args!("abandoning, operation {op}"));
        }
    }

    /// Forget `lineage`'s abandoned epochs at or below `through`, or all of them if `None`: the mark
    /// passed them, or the lineage ended or was retired. Each lets go of the place it pinned.
    pub(super) fn forget_abandoned(&mut self, lineage: Key, through: Option<E>) {
        let window = &mut self.window;
        let mut lost = None;
        self.abandoned.retain(|&(of, epoch), op| {
            if of != lineage || through.is_some_and(|through| epoch > through) {
                return true;
            }
            if let Err(what) = window.release(*op, Holder::Abandoned) {
                lost.get_or_insert((what, *op));
            }
            false
        });
        if let Some((what, op)) = lost {
            self.inconsistent(
                what,
                format_args!("lineage {lineage}, forgetting operation {op}"),
            );
        }
    }

    /// The places the abandoned epochs pin, for the census.
    #[cfg(test)]
    pub(super) fn abandoned_pins(&self) -> std::collections::BTreeMap<u64, u32> {
        self.abandoned.values().map(|&op| (op, 1)).collect()
    }

    /// Whether every abandoned epoch kept is of a live lineage and above its mark.
    #[cfg(test)]
    pub(super) fn abandoned_above_marks(&self) -> bool {
        self.abandoned.keys().all(|&(lineage, epoch)| {
            self.lineages.get(&lineage).is_some_and(|state| {
                !state.ended && state.durable_through.is_none_or(|mark| epoch > mark)
            })
        })
    }

    /// How many abandoned epochs are kept.
    #[cfg(test)]
    pub(crate) fn abandoned_kept(&self) -> usize {
        self.abandoned.len()
    }
}

/// Keep every epoch `failure` suspects a write of that is of a live lineage and above its mark, and
/// not kept already, pinning that write's place: what was inconsistent about it, if anything.
fn abandon<E: EpochId + 'static, T, I: Instant>(
    lists: &DomainLists,
    writes: &Ordered<u64, Write<E, T>>,
    kept: &Kept<E>,
    lineages: &Ordered<Key, LineageState<E>>,
    failure: &Failure<E, I>,
    abandoned: &mut Abandoned<E>,
    window: &mut Window,
) -> Option<(&'static str, u64)> {
    let mut lost = None;
    for write in suspects(lists, writes, kept, failure) {
        // An open failure holds every epoch it suspects, so the mark has passed none of them; the
        // check is for the lineage's end, and keeps the table's rule local. An ended lineage
        // answers for nothing (DI-D-30).
        let above = lineages
            .get(&write.lineage)
            .filter(|state| !state.ended)
            .is_some_and(|state| state.durable_through.is_none_or(|mark| write.epoch > mark));
        let key = (write.lineage, write.epoch);
        if !above || abandoned.get(&key).is_some() {
            continue;
        }
        let op = write.op.seq;
        if let Err(what) = window.hold(op, Holder::Abandoned) {
            lost.get_or_insert((what, op));
            continue;
        }
        if abandoned.insert(key, op).is_err() {
            lost.get_or_insert(("more epochs were abandoned than there are places", op));
        }
    }
    lost
}
