// Copyright (c) 2026 Mike Grier
//! The completion queue (DI-D-101, DI-D-112): made at construction for
//! [`Capacity::entries`](crate::capacity::Capacity::entries), which argues the bound, and pushed
//! onto only within it, a push past it being an inconsistency (DI-D-46).
//!
//! Two rules keep it within the bound. **An entry is withdrawn once a later one supersedes it**: a
//! `Durable` withdraws its lineage's older `Durable` (DI-D-101) and every `Blocked` of the lineage
//! at or below it, a `LineageEnded` every `Blocked` of its lineage, and a superseded empty seal its
//! own `Blocked`; retiring a live lineage withdraws its entries. **A lineage counts until its
//! `LineageEnded` is popped**, which the durability core keeps.

use super::{DurableIoCore, V};
use crate::capacity::within_queue;
use crate::clock::StampClock;
use crate::contract::{EpochId, Identities};
use crate::ids::Lineage;
use crate::types::{Entry, Epoch};

/// The lineage `entry` is about, for an entry about a lineage.
fn lineage_of<V: Identities, B, C>(entry: &Entry<V, B, C>) -> Option<V::Lineage> {
    match entry {
        Entry::Durable { through } | Entry::Blocked { through, .. } => Some(through.lineage),
        Entry::LineageEnded { lineage, .. } => Some(*lineage),
        _ => None,
    }
}

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// Put `entry` at the tail, within the room the queue was made with.
    pub(super) fn push_queued(&mut self, entry: Entry<V<E, K>, B, C>) {
        if within_queue(&mut self.queue, entry).is_err() {
            self.durability.inconsistent(
                "the completion queue held more entries than the declaration bounds",
                format_args!("instance {:?}", self.instance),
            );
        }
    }

    /// Append `entry`, first withdrawing what it supersedes. A lineage has at most one un-popped
    /// `Durable` (DI-D-101): a later one removes the older and goes to the tail, never written over
    /// it in place, since the contract has a `Healed` come before the `Durable` its heal lets
    /// through, and one updated in place could stand ahead of a `Healed` queued between them. It
    /// also withdraws every `Blocked` of its lineage at or below it, and a `LineageEnded` every
    /// `Blocked` of its lineage, whose seals are gone (DI-D-112). An entry withdrawn breaks no
    /// promise: the one superseding it is the later answer to the same request, so a consumer
    /// behind on popping sees the answer later, never a wrong one.
    pub(super) fn enqueue(&mut self, entry: Entry<V<E, K>, B, C>) {
        match &entry {
            Entry::Durable { through } => {
                let through = *through;
                self.queue.retain(|queued| match queued {
                    Entry::Durable { through: older } => older.lineage != through.lineage,
                    Entry::Blocked { through: held, .. } => {
                        held.lineage != through.lineage || held.id > through.id
                    }
                    _ => true,
                });
            }
            Entry::LineageEnded { lineage, .. } => {
                let lineage = *lineage;
                self.queue.retain(
                    |queued| !matches!(queued, Entry::Blocked { through, .. } if through.lineage == lineage),
                );
            }
            _ => {}
        }
        self.push_queued(entry);
    }

    /// Withdraw the un-popped `Blocked` of `lineage`'s seal through `through`, which a later empty
    /// seal replaced (DI-D-112).
    pub(super) fn withdraw_blocked(&mut self, through: Epoch<V<E, K>>) {
        self.queue.retain(
            |queued| !matches!(queued, Entry::Blocked { through: held, .. } if *held == through),
        );
    }

    /// Withdraw every un-popped entry about `lineage`, which the consumer retired (DI-D-112).
    pub(super) fn withdraw_lineage(&mut self, lineage: Lineage) {
        self.queue
            .retain(|queued| lineage_of(queued) != Some(lineage));
    }

    /// The next entry, if there is one. Popping an operation's completion lets go of the place the
    /// operation held in the submit window, popping a failure's `Abandoned` or `Healed` lets go of
    /// its records and what it held (DI-D-108), and popping a `LineageEnded` lets go of the
    /// lineage's count (DI-D-112).
    pub fn pop(&mut self) -> Option<Entry<V<E, K>, B, C>> {
        let entry = self.queue.pop_front()?;
        match &entry {
            Entry::Op(done) => self.durability.popped(done.id),
            Entry::Abandoned { failure } | Entry::Healed { failure } => {
                self.durability.forget(*failure);
            }
            Entry::LineageEnded { lineage, .. } => self.durability.end_popped(lineage.seq),
            _ => {}
        }
        #[cfg(test)]
        self.check_window();
        Some(entry)
    }

    /// How many places of the queue are taken.
    #[cfg(test)]
    pub(super) fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The room the queue was made with.
    #[cfg(test)]
    pub(super) fn queue_room(&self) -> usize {
        self.queue.capacity()
    }

    /// That the queue keeps the rules its bound rests on: within its room; at most one un-popped
    /// `Durable` for each lineage (DI-D-101); every lineage an entry names counted; and every
    /// `Blocked` naming a seal of a live lineage above its mark (DI-D-112).
    #[cfg(test)]
    pub(super) fn check_queue(&self) {
        assert!(
            self.queue.len() <= self.queue.capacity(),
            "the queue grew past its room"
        );
        let mut counted = alloc::collections::BTreeSet::new();
        self.durability.each_lineage(|key| {
            counted.insert(key);
        });
        let mut durables = alloc::collections::BTreeSet::new();
        for queued in &self.queue {
            if let Some(lineage) = lineage_of(queued) {
                assert!(
                    counted.contains(&lineage.seq),
                    "an entry names lineage {} which no longer counts",
                    lineage.seq
                );
            }
            match queued {
                Entry::Durable { through } => assert!(
                    durables.insert(through.lineage.seq),
                    "two un-popped Durable entries for lineage {}",
                    through.lineage.seq
                ),
                Entry::Blocked { through, .. } => {
                    let mark = self.durability.durable_through(through.lineage.seq);
                    assert!(
                        mark.is_some_and(|mark| mark.is_none_or(|mark| mark < through.id)),
                        "a Blocked through {through:?} outlived its seal"
                    );
                }
                _ => {}
            }
        }
    }
}
