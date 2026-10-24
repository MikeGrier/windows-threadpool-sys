// Copyright (c) 2026 Mike Grier
//! What the core holds of one file (DI-3.2.7): the part of `remove_target`'s refusal the core
//! answers, counted, and named through visitors (DI-D-103). Operations in flight and held for a
//! gate are the I/O core's to count.

use super::{Durability, Key, Stage, V, Write};
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::ids::FailureId;
use crate::types::{Epoch, IoTargetKey};

/// The core's holds on a file, counted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FileHolds {
    /// Writes to the file that completed and are not yet covered successfully, outside every
    /// failure's suspect set.
    pub(crate) uncovered: usize,
    /// Unresolved failures whose suspect sets include a write to the file.
    pub(crate) failures: usize,
}

impl FileHolds {
    /// Whether the core holds the file at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.uncovered == 0 && self.failures == 0
    }
}

impl<E: EpochId + 'static, T: Clone, I: Instant, K: StampClock<Instant = I>>
    Durability<E, T, I, K>
{
    /// What holds `file` here, counted. A write in flight is the I/O core's to count, and a write of
    /// an ended lineage is not counted once it has completed: the commit out for it is the I/O
    /// core's to count as well.
    pub(crate) fn file_holds(&self, file: IoTargetKey) -> FileHolds {
        FileHolds {
            uncovered: self
                .writes
                .values()
                .filter(|write| self.holds_uncovered(write, file))
                .count(),
            failures: self
                .failures
                .iter()
                .filter(|failure| failure.suspects_a_write_to(file))
                .count(),
        }
    }

    /// For each lineage with an uncovered write holding `file`, in lineage order, the highest epoch
    /// among them: what a seal must reach to clear it.
    pub(crate) fn file_uncovered(&self, file: IoTargetKey, mut visit: impl FnMut(Epoch<V<E, I>>)) {
        for &lineage in self.lineages.keys() {
            let highest = self
                .writes
                .values()
                .filter(|write| write.lineage == lineage && self.holds_uncovered(write, file))
                .map(|write| write.epoch)
                .max();
            if let Some(epoch) = highest {
                visit(Epoch::new(self.name(lineage), epoch));
            }
        }
    }

    /// Each unresolved failure whose suspect set includes a write to `file`, in observation order.
    pub(crate) fn file_failures(&self, file: IoTargetKey, mut visit: impl FnMut(FailureId)) {
        for failure in &self.failures {
            if failure.suspects_a_write_to(file) {
                visit(failure.id);
            }
        }
    }

    /// Whether `write` holds `file` as uncovered: a completed write to it, of a live lineage,
    /// suspected by no unresolved failure, which holds it instead.
    fn holds_uncovered(&self, write: &Write<E, T>, file: IoTargetKey) -> bool {
        write.file == file
            && write.stage != Stage::InFlight
            && self
                .lineages
                .get(&write.lineage)
                .is_some_and(|state| !state.ended)
            && !self
                .failures
                .iter()
                .any(|failure| failure.suspects(write.op))
    }

    /// Each unresolved failure holding `lineage`, in observation order.
    pub(crate) fn lineage_failures(&self, lineage: Key, mut visit: impl FnMut(FailureId)) {
        for failure in &self.failures {
            if failure.holds(lineage).is_some() {
                visit(failure.id);
            }
        }
    }
}
