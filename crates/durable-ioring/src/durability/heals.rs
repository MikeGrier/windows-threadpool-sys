// Copyright (c) 2026 Mike Grier
//! When a heal takes effect in a lineage (DI-D-41, DI-D-46, DI-D-114), read from the write records
//! rather than kept for each lineage a heal waits on.
//!
//! A heal records only when it was made, as a [`HealAt`]: the push sequence and the seal clock
//! then. A seal records the seal clock when it was made. What a heal must reach in a lineage is
//! read when a seal of it finishes: the highest epoch among the lineage's writes pushed before
//! the heal that are still to be committed -- in flight, uncovered, or held for a gate. A write
//! already covered, or that failed, has nothing left to commit, and one at or below an earlier
//! seal point is reached by any later seal; the consumer learns of a failed write from its own
//! completion, and an observer of the failures it suspected (DI-D-113).
//!
//! **The heals a seal releases are a prefix, so a lineage keeps one value.** A heal made earlier
//! was made no later than the seal, and what it must reach is read over a subset of the writes a
//! later heal's is read over, so it waits for no more. A seal that releases a heal releases every
//! heal made before it, and a lineage keeps only the latest heal released. A heal is pending on a
//! lineage while the lineage is live and has not released it; ended, it no longer holds the heal
//! back (DI-D-30).

use super::{Durability, Failure, HealAt, Key, Standing};
use crate::clock::Instant;
use crate::contract::EpochId;

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// What a heal made at `at` must reach in `lineage`: the highest epoch among its writes pushed
    /// before it that are still to be committed, or `None` if there are none.
    pub(super) fn heal_reach(&self, lineage: Key, at: HealAt) -> Option<E> {
        let recorded = self
            .writes
            .values()
            .filter(|write| write.lineage == lineage && write.op.seq < at.pushes)
            .map(|write| write.epoch);
        let held = self
            .held
            .values()
            .filter(|held| held.op.seq < at.pushes)
            .filter_map(|held| held.write.as_ref())
            .filter(|write| write.lineage == lineage)
            .map(|write| write.epoch);
        recorded.chain(held).max()
    }

    /// Whether a seal of `lineage` made at `made` through `through`, finished successfully,
    /// releases a heal made at `at`.
    pub(super) fn releases(&self, lineage: Key, made: u64, through: E, at: HealAt) -> bool {
        made >= at.seals
            && self
                .heal_reach(lineage, at)
                .is_none_or(|reach| through >= reach)
    }

    /// A seal of `lineage` made at `made` through `through` finished with every part successful:
    /// note the latest heal it releases, and with it every heal made before.
    pub(super) fn release_heals(&mut self, lineage: Key, made: u64, through: E) {
        let latest = self
            .failures
            .iter()
            .filter_map(|failure| match failure.standing {
                Standing::Healing { at } => Some(at),
                Standing::Open | Standing::Resolved => None,
            })
            .filter(|&at| self.releases(lineage, made, through, at))
            .max();
        // `latest` is never below what this lineage released before for a heal still healing: a
        // later seal of it is made later and reaches further, and what a heal waits for only
        // shrinks, so it releases every such heal again; one healed since reads nothing here. The
        // `max` says the value only grows rather than relying on that; the sabotage dropping it is a
        // control that survives.
        if let Some(state) = self.lineages.get_mut(&lineage) {
            state.released = state.released.max(latest);
        }
    }

    /// Whether `failure` still holds `lineage`'s epochs among its suspects': open, or healing and
    /// pending on it -- the lineage live, and its latest heal released made before this one.
    pub(super) fn pending_on(&self, failure: &Failure<E, I>, lineage: Key) -> bool {
        match failure.standing {
            Standing::Open => true,
            Standing::Healing { at } => self
                .lineages
                .get(&lineage)
                .is_some_and(|state| !state.ended && state.released.is_none_or(|done| done < at)),
            Standing::Resolved => false,
        }
    }

    /// When a heal made now is made.
    pub(super) fn heal_now(&self) -> HealAt {
        HealAt {
            pushes: self.next_push,
            seals: self.seals_made,
        }
    }
}
