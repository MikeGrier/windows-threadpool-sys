// Copyright (c) 2026 Mike Grier
//! The core's lineages (DI-D-19, DI-D-30, DI-D-40): minting, enumerating, ending and retiring.
//!
//! The core takes a lineage by its plain name, so a lineage handle's rules -- ending with its last
//! copy, retiring only through the last -- are the driver's. A lineage this core cannot answer for,
//! another core's or one already ended or retired, is refused as `Foreign`.

use super::{Actions, DurableIoCore, V};
use crate::capacity::{AtCapacity, Limit};
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::durability::DEFAULT_LINEAGE;
use crate::ids::Lineage;
use crate::types::{EndLineageRefusal, Hold, LineageInfo, RetireLineageRefusal};

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// Mint a lineage, described by `description`.
    ///
    /// # Errors
    ///
    /// [`AtCapacity`] when the core holds as many lineages as it was built for, the default and
    /// those ended and not yet retired among them (DI-D-107); nothing is minted.
    pub fn mint_lineage(&mut self, description: Option<String>) -> Result<Lineage, AtCapacity> {
        if self.durability.lineage_count() >= self.capacity.lineages {
            return Err(AtCapacity {
                limit: Limit::Lineages,
                declared: self.capacity.lineages,
            });
        }
        let seq = self.durability.mint(description);
        Ok(self.name(seq))
    }

    /// Each live lineage, in minting order, the default first, given to `visit` (DI-D-103).
    pub fn lineages(&self, visit: impl FnMut(LineageInfo<V<E, K>>)) {
        self.durability.lineages(visit);
    }

    /// What holds `lineage` from being retired that a [`LineageBusy`](crate::LineageBusy) counts
    /// without naming, given to `visit` (DI-D-103): each unresolved failure holding it, in the
    /// order observed. Nothing for a lineage that is not this core's live one.
    pub fn lineage_holds(&self, lineage: Lineage, mut visit: impl FnMut(Hold<V<E, K>>)) {
        if self.is_live(lineage) {
            self.durability
                .lineage_failures(lineage.seq, |failure| visit(Hold::Failure(failure)));
        }
    }

    /// End `lineage` (DI-D-30): every epoch of it not yet durable is abandoned, a `LineageEnded`
    /// entry reports it, and every operation held by a gate on one of its epochs, or a held write
    /// of its own, completes as `NeverIssued`.
    ///
    /// # Errors
    ///
    /// [`EndLineageRefusal::Foreign`] for a lineage that is not this core's live one, and
    /// [`EndLineageRefusal::Default`] for the default lineage.
    pub fn end_lineage(
        &mut self,
        lineage: Lineage,
    ) -> Result<Actions<E, B, K>, EndLineageRefusal<V<E, K>>> {
        if !self.is_live(lineage) {
            return Err(EndLineageRefusal::Foreign(lineage));
        }
        if lineage.seq == DEFAULT_LINEAGE {
            return Err(EndLineageRefusal::Default);
        }
        let was_empty = self.queue.is_empty();
        let due = self.durability.end(lineage.seq);
        Ok(self.act(due, was_empty))
    }

    /// Retire `lineage` if nothing holds it, so it is no longer enumerated (DI-D-40).
    ///
    /// # Errors
    ///
    /// [`RetireLineageRefusal::Foreign`] for a lineage that is not this core's live one,
    /// [`RetireLineageRefusal::Default`] for the default lineage, and
    /// [`RetireLineageRefusal::Busy`] naming what still holds it.
    pub fn retire_lineage(
        &mut self,
        lineage: Lineage,
    ) -> Result<(), RetireLineageRefusal<V<E, K>>> {
        if !self.is_live(lineage) {
            return Err(RetireLineageRefusal::Foreign(lineage));
        }
        if lineage.seq == DEFAULT_LINEAGE {
            return Err(RetireLineageRefusal::Default);
        }
        self.durability
            .retire(lineage.seq)
            .map_err(RetireLineageRefusal::Busy)
    }
}
