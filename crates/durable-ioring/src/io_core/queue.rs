// Copyright (c) 2026 Mike Grier
//! The completion queue (DI-D-101, DI-D-109, DI-D-112): made at construction for
//! [`Capacity::entries`](crate::capacity::Capacity::entries), which argues the bound, and pushed
//! onto only within it, a push past it being an inconsistency (DI-D-46).
//!
//! Three rules keep it within the bound. **A marking is queued once per write** and expanded when
//! popped into one `Marked` for each failure it goes to, read from the write's kept record and the
//! failure table. **An entry is withdrawn once a later one supersedes it**: a `Durable` withdraws
//! its lineage's older `Durable` (DI-D-101) and every `Blocked` of the lineage at or below it, a
//! `LineageEnded` every `Blocked` of its lineage, and a superseded empty seal its own `Blocked`;
//! retiring a live lineage withdraws its entries. **A lineage counts until its `LineageEnded` is
//! popped**, which the durability core keeps.

#[cfg(test)]
use alloc::collections::VecDeque;

use super::{DurableIoCore, V};
use crate::capacity::within_queue;
use crate::clock::StampClock;
use crate::contract::{EpochId, Identities};
#[cfg(test)]
use crate::ids::FailureId;
use crate::ids::{Lineage, OpId};
#[cfg(test)]
use crate::types::Marking;
use crate::types::{Entry, Epoch};

/// One place in the completion queue.
pub(super) enum Queued<V: Identities, B, C> {
    /// An entry, popped as it is.
    Entry(Entry<V, B, C>),
    /// A suspected write's marking, popped as one `Marked` for each failure it goes to.
    Marked {
        write: OpId,
        /// The number of the failure last given it, if any has been.
        after: Option<u64>,
        /// What the expansion must still give, as the durability core saw it when the write was
        /// marked.
        #[cfg(test)]
        to: VecDeque<(FailureId, Marking<V>)>,
    },
}

impl<V: Identities, B, C> Queued<V, B, C> {
    /// The lineage the entry is about, for an entry about a lineage.
    fn lineage(&self) -> Option<V::Lineage> {
        match self {
            Queued::Entry(Entry::Durable { through } | Entry::Blocked { through, .. }) => {
                Some(through.lineage)
            }
            Queued::Entry(Entry::LineageEnded { lineage, .. }) => Some(*lineage),
            _ => None,
        }
    }
}

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// Put `queued` at the tail, within the room the queue was made with.
    pub(super) fn push_queued(&mut self, queued: Queued<V<E, K>, B, C>) {
        if within_queue(&mut self.queue, queued).is_err() {
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
                    Queued::Entry(Entry::Durable { through: older }) => {
                        older.lineage != through.lineage
                    }
                    Queued::Entry(Entry::Blocked { through: held, .. }) => {
                        held.lineage != through.lineage || held.id > through.id
                    }
                    _ => true,
                });
            }
            Entry::LineageEnded { lineage, .. } => {
                let lineage = *lineage;
                self.queue.retain(|queued| {
                    !matches!(queued, Queued::Entry(Entry::Blocked { through, .. })
                        if through.lineage == lineage)
                });
            }
            _ => {}
        }
        self.push_queued(Queued::Entry(entry));
    }

    /// Withdraw the un-popped `Blocked` of `lineage`'s seal through `through`, which a later empty
    /// seal replaced (DI-D-112).
    pub(super) fn withdraw_blocked(&mut self, through: Epoch<V<E, K>>) {
        self.queue.retain(|queued| {
            !matches!(queued, Queued::Entry(Entry::Blocked { through: held, .. }) if *held == through)
        });
    }

    /// Withdraw every un-popped entry about `lineage`, which the consumer retired (DI-D-112).
    pub(super) fn withdraw_lineage(&mut self, lineage: Lineage) {
        self.queue
            .retain(|queued| queued.lineage() != Some(lineage));
    }

    /// The next entry, if there is one. Popping an operation's completion lets go of the place the
    /// operation held in the submit window, popping a failure's `Abandoned` or `Healed` lets go of
    /// its records and what it held (DI-D-108), and popping a `LineageEnded` lets go of the
    /// lineage's count (DI-D-112).
    pub fn pop(&mut self) -> Option<Entry<V<E, K>, B, C>> {
        loop {
            let Queued::Marked { write, after, .. } = self.queue.front()? else {
                let Some(Queued::Entry(entry)) = self.queue.pop_front() else {
                    unreachable!("the front is an entry");
                };
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
                return Some(entry);
            };
            let (write, after) = (*write, *after);
            let Some((failure, marking, more)) = self.durability.next_marking(write, after) else {
                self.durability.inconsistent(
                    "a queued marking went to no failure",
                    format_args!("write {write:?}, after failure {after:?}"),
                );
                self.queue.pop_front();
                continue;
            };
            #[cfg(test)]
            if let Some(Queued::Marked { to, .. }) = self.queue.front_mut() {
                assert_eq!(
                    to.pop_front(),
                    Some((failure, marking.clone())),
                    "a queued marking expanded otherwise than it was made"
                );
                assert_eq!(to.is_empty(), !more, "a queued marking ended early or late");
            }
            if more {
                if let Some(Queued::Marked { after, .. }) = self.queue.front_mut() {
                    *after = Some(failure.seq);
                }
            } else {
                self.queue.pop_front();
            }
            return Some(Entry::Marked { failure, marking });
        }
    }

    /// How many places of the queue are taken: a queued marking is one, however many failures it
    /// goes to.
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
            if let Some(lineage) = queued.lineage() {
                assert!(
                    counted.contains(&lineage.seq),
                    "an entry names lineage {} which no longer counts",
                    lineage.seq
                );
            }
            match queued {
                Queued::Entry(Entry::Durable { through }) => assert!(
                    durables.insert(through.lineage.seq),
                    "two un-popped Durable entries for lineage {}",
                    through.lineage.seq
                ),
                Queued::Entry(Entry::Blocked { through, .. }) => {
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
