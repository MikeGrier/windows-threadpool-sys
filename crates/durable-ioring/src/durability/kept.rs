// Copyright (c) 2026 Mike Grier
//! Writes no longer uncovered that an unresolved failure still suspects (DI-D-108): kept, with the
//! marking each gained when it stopped being uncovered, so a failure keeps no copy of its own.
//!
//! A write stops being uncovered once -- covered, nullified by its own failed completion, or let
//! go with its ended lineage -- and is marked at most once, at that moment: covered or nullified,
//! never both, and a write let go with its lineage not at all. Every failure suspecting it then
//! shares the one marking, which is what `mark` gave each of them anyway: one reading of the clock
//! and one kind. A record is kept while any unresolved failure suspects its write, which holds the
//! write's place in the window (DI-D-100), so the records are at most the places, at most the
//! operations declared. The markings are kept in a second table in the order observed, so a
//! failure's are read in that order.
//!
//! A record keeps what a failure's rule reads (DI-D-109): the write's lineage, epoch, file and
//! domains, and the failure clock's readings when it was recorded and when it stopped being
//! nameable, so whether a failure suspects it is decided the same way kept or uncovered.

use super::Key;
use crate::capacity::{ConstructionError, ordered};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::domain_lists::DomainList;
use crate::ids::{DioringIds, OpId};
use crate::ordered::Ordered;
use crate::types::{IoTargetKey, Marking, MarkingKind};

#[cfg(test)]
mod tests;

/// A write that has stopped being uncovered, as a failure's rule reads it.
pub(super) struct Gone<E> {
    pub(super) op: OpId,
    pub(super) lineage: Key,
    pub(super) epoch: E,
    pub(super) file: IoTargetKey,
    pub(super) domains: DomainList,
    /// The failure clock when it was recorded.
    pub(super) born: u64,
    /// The failure clock when it stopped being nameable: no failure observed since names it.
    pub(super) until: u64,
}

/// One kept write.
pub(super) struct Record<E, I> {
    pub(super) write: Gone<E>,
    /// Its marking, if it gained one: its place in the order observed, when, and what.
    marking: Option<(u64, I, MarkingKind)>,
}

/// The kept writes, by push sequence, and their markings in the order observed.
pub(super) struct Kept<E, I> {
    records: Ordered<u64, Record<E, I>>,
    /// The write each marking is of, by its place in the order observed.
    marked: Ordered<u64, u64>,
    next: u64,
}

impl<E: EpochId + 'static, I: Instant> Kept<E, I> {
    /// Room for a record for each of `operations` places.
    pub(super) fn new(operations: usize) -> Result<Self, ConstructionError> {
        Ok(Self {
            records: ordered("the kept writes", Some(operations))?,
            marked: ordered("the kept writes' markings", Some(operations))?,
            next: 0,
        })
    }

    /// Keep `write`, which has stopped being uncovered while a failure suspects it.
    ///
    /// # Errors
    ///
    /// What about it is inconsistent: it is kept already, or there is no room for it.
    pub(super) fn keep(&mut self, write: Gone<E>) -> Result<(), &'static str> {
        let seq = write.op.seq;
        if self.records.get(&seq).is_some() {
            return Err("a write was kept twice");
        }
        self.records
            .insert(
                seq,
                Record {
                    write,
                    marking: None,
                },
            )
            .map(|_| ())
            .map_err(|_| "more writes were kept than there are places")
    }

    /// Kept write `op` was marked, `observed` then, with `kind`.
    ///
    /// # Errors
    ///
    /// What about it is inconsistent: the write is not kept, or was marked already.
    pub(super) fn mark(
        &mut self,
        op: OpId,
        observed: I,
        kind: MarkingKind,
    ) -> Result<(), &'static str> {
        let Some(record) = self.records.get_mut(&op.seq) else {
            return Err("a suspected write was marked that was not kept");
        };
        if record.marking.is_some() {
            return Err("a kept write was marked twice");
        }
        let order = self.next;
        if self.marked.insert(order, op.seq).is_err() {
            return Err("more kept writes were marked than there are places");
        }
        self.next += 1;
        record.marking = Some((order, observed, kind));
        Ok(())
    }

    /// Let go of every record `gone` picks: no failure suspects its write any more.
    pub(super) fn release_where(&mut self, mut gone: impl FnMut(&Gone<E>) -> bool) {
        let marked = &mut self.marked;
        self.records.retain(|_, record| {
            if !gone(&record.write) {
                return true;
            }
            if let Some((order, ..)) = record.marking {
                marked.remove(&order);
            }
            false
        });
    }

    /// The marking of the kept write pushed `seq`th, if it is kept and marked: its place in the
    /// order observed, when, and what.
    pub(super) fn marking_of(&self, seq: u64) -> Option<(u64, I, &MarkingKind)> {
        let (order, observed, kind) = self.records.get(&seq)?.marking.as_ref()?;
        Some((*order, *observed, kind))
    }

    /// The kept write pushed `seq`th, if it is kept.
    pub(super) fn get(&self, seq: u64) -> Option<&Gone<E>> {
        self.records.get(&seq).map(|record| &record.write)
    }

    /// The kept writes, by push sequence.
    pub(super) fn writes(&self) -> impl Iterator<Item = &Gone<E>> + Clone {
        self.records.values().map(|record| &record.write)
    }

    /// The place in the order observed the next marking will take.
    pub(super) fn order(&self) -> u64 {
        self.next
    }

    /// Every marking of a write `suspects` picks, in the order observed: only those made before
    /// `before` in that order, if given.
    pub(super) fn markings<'a>(
        &'a self,
        suspects: impl Fn(OpId) -> bool + 'a,
        before: Option<u64>,
    ) -> impl Iterator<Item = Marking<DioringIds<E, I>>> + 'a {
        self.marked
            .iter()
            .take_while(move |(order, _)| before.is_none_or(|before| **order < before))
            .filter_map(|(_, op)| self.records.get(op))
            .filter(move |record| suspects(record.write.op))
            .filter_map(|record| {
                let (_, observed, kind) = record.marking.as_ref()?;
                Some(Marking {
                    write: record.write.op,
                    observed: *observed,
                    kind: kind.clone(),
                })
            })
    }

    /// The kept writes, by push sequence.
    #[cfg(test)]
    pub(super) fn ops(&self) -> impl Iterator<Item = u64> + '_ {
        self.records.keys().copied()
    }
}
