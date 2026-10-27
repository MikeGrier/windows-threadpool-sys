// Copyright (c) 2026 Mike Grier
//! Writes no longer uncovered that an unresolved failure still suspects (DI-D-108): kept, so a
//! failure keeps no copy of its own.
//!
//! A write stops being uncovered once -- covered, nullified by its own failed completion, or let
//! go with its ended lineage. A record is kept while any unresolved failure suspects its write,
//! which holds the write's place in the window (DI-D-100), so the records are at most the places,
//! at most the operations declared. The marking a write gains as it stops being uncovered is told
//! to the failure observer and kept nowhere (DI-D-113).
//!
//! A record keeps what a failure's rule reads (DI-D-109): the write's lineage, epoch, file and
//! domains, and the failure clock's readings when it was recorded and when it stopped being
//! nameable, so whether a failure suspects it is decided the same way kept or uncovered.

use super::Key;
use crate::capacity::{ConstructionError, ordered};
use crate::contract::EpochId;
use crate::domain_lists::DomainList;
use crate::ids::OpId;
use crate::ordered::Ordered;
use crate::types::IoTargetKey;

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

/// The kept writes, by push sequence.
pub(super) struct Kept<E> {
    records: Ordered<u64, Gone<E>>,
}

impl<E: EpochId + 'static> Kept<E> {
    /// Room for a record for each of `operations` places.
    pub(super) fn new(operations: usize) -> Result<Self, ConstructionError> {
        Ok(Self {
            records: ordered("the kept writes", Some(operations))?,
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
            .insert(seq, write)
            .map(|_| ())
            .map_err(|_| "more writes were kept than there are places")
    }

    /// Let go of every record `gone` picks: no failure suspects its write any more.
    pub(super) fn release_where(&mut self, mut gone: impl FnMut(&Gone<E>) -> bool) {
        self.records.retain(|_, write| !gone(write));
    }

    /// The kept write pushed `seq`th, if it is kept.
    pub(super) fn get(&self, seq: u64) -> Option<&Gone<E>> {
        self.records.get(&seq)
    }

    /// The kept writes, by push sequence.
    pub(super) fn writes(&self) -> impl Iterator<Item = &Gone<E>> + Clone {
        self.records.values()
    }

    /// The kept writes, by push sequence.
    #[cfg(test)]
    pub(super) fn ops(&self) -> impl Iterator<Item = u64> + '_ {
        self.records.keys().copied()
    }
}
