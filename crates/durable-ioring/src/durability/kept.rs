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

use crate::capacity::{ConstructionError, ordered};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::ids::{DioringIds, OpId};
use crate::ordered::Ordered;
use crate::types::{Marking, MarkingKind};

#[cfg(test)]
mod tests;

/// One kept write.
struct Record<I> {
    op: OpId,
    /// Its marking, if it gained one: its place in the order observed, when, and what.
    marking: Option<(u64, I, MarkingKind)>,
}

/// The kept writes, by push sequence, and their markings in the order observed.
pub(super) struct Kept<I> {
    records: Ordered<u64, Record<I>>,
    /// The write each marking is of, by its place in the order observed.
    marked: Ordered<u64, u64>,
    next: u64,
}

impl<I: Instant> Kept<I> {
    /// Room for a record for each of `operations` places.
    pub(super) fn new(operations: usize) -> Result<Self, ConstructionError> {
        Ok(Self {
            records: ordered("the kept writes", Some(operations))?,
            marked: ordered("the kept writes' markings", Some(operations))?,
            next: 0,
        })
    }

    /// Keep write `op`, which has stopped being uncovered while a failure suspects it.
    ///
    /// # Errors
    ///
    /// What about it is inconsistent: it is kept already, or there is no room for it.
    pub(super) fn keep(&mut self, op: OpId) -> Result<(), &'static str> {
        if self.records.get(&op.seq).is_some() {
            return Err("a write was kept twice");
        }
        self.records
            .insert(op.seq, Record { op, marking: None })
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

    /// Let go of write `op`'s record, if it is kept: no failure suspects it any more.
    pub(super) fn release(&mut self, op: u64) {
        if let Some(Record {
            marking: Some((order, ..)),
            ..
        }) = self.records.remove(&op)
        {
            self.marked.remove(&order);
        }
    }

    /// Every marking of a write `suspects` picks, in the order observed.
    pub(super) fn markings<'a, E: EpochId + 'static>(
        &'a self,
        suspects: impl Fn(OpId) -> bool + 'a,
    ) -> impl Iterator<Item = Marking<DioringIds<E, I>>> + 'a {
        self.marked
            .values()
            .filter_map(|op| self.records.get(op))
            .filter(move |record| suspects(record.op))
            .filter_map(|record| {
                let (_, observed, kind) = record.marking.as_ref()?;
                Some(Marking {
                    write: record.op,
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
