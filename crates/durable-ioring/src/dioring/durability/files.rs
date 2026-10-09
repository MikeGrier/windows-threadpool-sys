// Copyright (c) 2026 Mike Grier
//! What the core holds of one file (DI-3.2.7): the part of `remove_target`'s refusal the core
//! answers. Operations in flight and held for a gate are the relay's to count.

use std::collections::{BTreeMap, BTreeSet};

use super::{Durability, Key, Stage, V};
use crate::contract::EpochId;
use crate::dioring::TimeBase;
use crate::ids::FailureId;
use crate::types::{Epoch, IoTargetKey};

/// The core's holds on a file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FileHolds<E: EpochId + 'static> {
    /// Writes to the file that completed and are not yet covered successfully, outside every
    /// failure's suspect set.
    pub(crate) uncovered: usize,
    /// The highest epoch among those writes, in each lineage that has one, in lineage order.
    pub(crate) uncovered_through: Vec<Epoch<V<E>>>,
    /// Unresolved failures whose suspect sets include a write to the file, in observation order.
    pub(crate) failures: Vec<FailureId>,
}

impl<E: EpochId + 'static> FileHolds<E> {
    /// Whether the core holds the file at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.uncovered == 0 && self.failures.is_empty()
    }
}

impl<E: EpochId + 'static, T: Clone, K: TimeBase> Durability<E, T, K> {
    /// What holds `file` here. A write in flight is the relay's to count, and a write of an ended
    /// lineage is no longer recorded once it has completed.
    pub(crate) fn file_holds(&self, file: IoTargetKey) -> FileHolds<E> {
        let mut suspect = BTreeSet::new();
        let mut failures = Vec::new();
        for failure in &self.failures {
            let mut names_file = false;
            for write in failure.suspect.writes().iter().filter(|w| w.target == file) {
                names_file = true;
                suspect.insert(write.op.seq);
            }
            if names_file {
                failures.push(failure.id);
            }
        }
        let mut uncovered = 0;
        let mut through: BTreeMap<Key, E> = BTreeMap::new();
        for write in self.writes.values().filter(|write| {
            write.file == file && write.stage != Stage::InFlight && !suspect.contains(&write.op.seq)
        }) {
            uncovered += 1;
            through
                .entry(write.lineage)
                .and_modify(|highest| *highest = (*highest).max(write.epoch))
                .or_insert(write.epoch);
        }
        FileHolds {
            uncovered,
            uncovered_through: through
                .into_iter()
                .map(|(lineage, epoch)| Epoch::new(self.name(lineage), epoch))
                .collect(),
            failures,
        }
    }
}
