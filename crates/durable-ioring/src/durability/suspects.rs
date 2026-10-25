// Copyright (c) 2026 Mike Grier
//! Which writes a failure suspects, read from the write records by the rule observation applied
//! (DI-D-109): a failure keeps no list of its own.
//!
//! Observation names every uncovered write of a live lineage -- of the lineage it was confined to,
//! if any -- whose file its reach covers, frozen then (DI-D-12 (b)). Each write record carries the
//! failure clock's reading when it was recorded and when it stopped being nameable -- covered,
//! nullified, let go, or its lineage ended, whichever came first -- so a failure numbered `n`
//! names exactly the writes recorded at or before `n` and nameable after it that its confinement
//! and its reach pass. A write that stops being uncovered while some failure names it is kept with
//! those readings ([`Kept`]), so the rule reads the same whether the write is uncovered or kept.
//! The epochs a failure holds, and the lineages it holds them in, are its suspects', read the same
//! way.

use alloc::vec::Vec;
use core::ops::RangeBounds;

use super::kept::Kept;
use super::{Durability, Failure, Key, V, Write};
use crate::alloc_tags::{Tag, tagged};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::ids::{Lineage, OpId};
use crate::ordered::Ordered;
use crate::types::{Epoch, IoTargetKey, SuspectSet, SuspectWrite};

/// A write a failure suspects, as its rule reads it.
#[derive(Clone, Copy)]
pub(super) struct Suspect<E> {
    pub(super) op: OpId,
    pub(super) lineage: Key,
    pub(super) epoch: E,
    pub(super) file: IoTargetKey,
}

/// Every write `failure` suspects: the uncovered ones, then the kept ones, each in push order.
pub(super) fn suspects<'a, E: EpochId + 'static, T, I: Instant>(
    writes: &'a Ordered<u64, Write<E, T>>,
    kept: &'a Kept<E, I>,
    failure: &'a Failure<E, I>,
) -> impl Iterator<Item = Suspect<E>> + 'a {
    let uncovered = writes
        .values()
        .filter(move |write| failure.names(write.born, write.until, write.lineage, &write.domains))
        .map(|write| Suspect {
            op: write.op,
            lineage: write.lineage,
            epoch: write.epoch,
            file: write.file,
        });
    let gone = kept
        .writes()
        .filter(move |write| {
            failure.names(write.born, Some(write.until), write.lineage, &write.domains)
        })
        .map(|write| Suspect {
            op: write.op,
            lineage: write.lineage,
            epoch: write.epoch,
            file: write.file,
        });
    uncovered.chain(gone)
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// Whether `failure` suspects write `op`.
    pub(super) fn suspects(&self, failure: &Failure<E, I>, op: OpId) -> bool {
        if let Some(write) = self.writes.get(&op.seq) {
            return write.op == op
                && failure.names(write.born, write.until, write.lineage, &write.domains);
        }
        self.kept.get(op.seq).is_some_and(|write| {
            write.op == op
                && failure.names(write.born, Some(write.until), write.lineage, &write.domains)
        })
    }

    /// Whether `failure` suspects a write to `file`.
    pub(super) fn suspects_a_write_to(&self, failure: &Failure<E, I>, file: IoTargetKey) -> bool {
        suspects(&self.writes, &self.kept, failure).any(|write| write.file == file)
    }

    /// Whether `failure` holds an epoch of `lineage` within `epochs`: it suspects a write of it
    /// there, and is open, or healing and still pending on the lineage.
    pub(super) fn holds_within(
        &self,
        failure: &Failure<E, I>,
        lineage: Key,
        epochs: impl RangeBounds<E>,
    ) -> bool {
        failure.pending(lineage)
            && suspects(&self.writes, &self.kept, failure)
                .any(|write| write.lineage == lineage && epochs.contains(&write.epoch))
    }

    /// `failure`'s suspect set as the entries carry it, in push order, collected under `tag`
    /// (until DI-23.12.3.4 lends it instead).
    pub(super) fn suspect_set(&self, failure: &Failure<E, I>, tag: Tag) -> SuspectSet<V<E, I>> {
        let instance = self.instance;
        let mut writes: Vec<SuspectWrite<V<E, I>>> = tagged(tag, || {
            suspects(&self.writes, &self.kept, failure)
                .map(|write| SuspectWrite {
                    op: write.op,
                    target: write.file,
                    epoch: Epoch::new(
                        Lineage {
                            instance,
                            seq: write.lineage,
                        },
                        write.epoch,
                    ),
                })
                .collect()
        });
        writes.sort_unstable_by_key(|write| write.op.seq);
        SuspectSet::new(writes)
    }
}
