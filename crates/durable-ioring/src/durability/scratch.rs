// Copyright (c) 2026 Mike Grier
//! The durability core's per-call storage, made at construction and reused by every call (DI-D-97,
//! DI-23.12.2): the lists a change makes due into, and the scratch its searches and walks collect
//! into. Each list is reserved for a bound on what one call can put in it, stated beside it, and is
//! pushed onto only through [`within`](crate::capacity::within), so a push past the bound is
//! reported as an inconsistency rather than grown into.
//!
//! The lists a change makes due -- a [`Due`] -- are lent out by [`Durability::fresh`] and given
//! back by [`Durability::recycle`] once the I/O core has acted on them. One is out at a time: the
//! I/O core acts on each change's before it makes the next. A change made while they are out gets
//! lists of its own, made then at the same sizes, which is an allocation after construction and
//! never a list too small; the I/O core's tests check after every call that the lists came back,
//! and the allocation test counts any made.

use alloc::vec::Vec;

use super::{Due, Durability, FlushPair, Gate, Key, MarkingKind, Part};
use crate::alloc_tags::{tag, tagged};
use crate::capacity::{Capacity, ConstructionError, list};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::ids::OpId;

use super::gates::Step;

/// The scratch a call collects into, emptied when the call is done with it.
pub(super) struct Scratch<E> {
    /// The live lineages `advance` walks: at most the lineages declared.
    pub(super) live: Vec<Key>,
    /// The writes a seal's answers cover, in push order: uncovered writes, at most the operations
    /// declared.
    pub(super) covered: Vec<(OpId, MarkingKind)>,
    /// The gates a cycle search has still to look at: at most the operations declared. A search
    /// scans each lineage's gated writes once, and never the sealed lineage's, which the seeds
    /// are: reaching their range is reaching the goal. So every gate on the list is a distinct
    /// gated write's, but a lone seed, which is taken off before anything is scanned.
    pub(super) work: Vec<Gate<E>>,
    /// A cycle search's bound for each lineage it reached: at most the lineages declared.
    pub(super) bounds: Vec<(Key, E)>,
    /// A traced cycle's steps: the first, and each gated write at most once -- at most the
    /// operations declared, plus one.
    pub(super) steps: Vec<Step<E>>,
    /// The flush pairs `end_pairs` ends, sorted before they are: at most the flush parts.
    pub(super) pairs: Vec<((Key, u64, Part), FlushPair<E>)>,
}

impl<E> Scratch<E> {
    /// Scratch for a core built for `capacity`, with a provider serving `served` domains.
    pub(super) fn new(capacity: &Capacity, served: usize) -> Result<Self, ConstructionError> {
        let operations = capacity.operations;
        Ok(Self {
            live: tagged(tag::DURABILITY_NEW_SCRATCH_LIVE, || {
                list("the live lineages a change walks", Some(capacity.lineages))
            })?,
            covered: tagged(tag::DURABILITY_NEW_SCRATCH_COVERED, || {
                list("the writes a seal's answers cover", Some(operations))
            })?,
            work: tagged(tag::DURABILITY_NEW_SCRATCH_WORK, || {
                list("the gates a cycle search visits", Some(operations))
            })?,
            bounds: tagged(tag::DURABILITY_NEW_SCRATCH_BOUNDS, || {
                list("a cycle search's bounds", Some(capacity.lineages))
            })?,
            steps: tagged(tag::DURABILITY_NEW_SCRATCH_STEPS, || {
                list("a traced cycle's steps", operations.checked_add(1))
            })?,
            pairs: tagged(tag::DURABILITY_NEW_SCRATCH_PAIRS, || {
                list(
                    "the flush pairs an end closes",
                    capacity.flush_parts(served),
                )
            })?,
        })
    }
}

impl<E: EpochId + 'static, T, I: Instant> Due<E, T, I> {
    /// Lists with room for what one change can make due, for a core built for `capacity`.
    ///
    /// Flushes: one for each span entry a seal starts flushing, and entries are at most the
    /// operations declared. Releases: one for each held operation, at most the operations.
    /// Provider requests: one for each seal that starts, and recorded seals are at most the
    /// operations (DI-D-110). Entries: on the clean paths, a `Durable` or a `Blocked` for each seal
    /// the mark passes or holds -- recorded seals, and empty ones, one leading each lineage and one
    /// trailing each recorded seal -- a `NeverIssued` for each held operation, and one
    /// `LineageEnded`: three times the operations, plus the lineages, plus one. The entries a
    /// failure makes are bounded with its storage (DI-23.12.3); until then a change making more is
    /// let grow the list, under its tag.
    pub(super) fn reserved(capacity: &Capacity) -> Result<Self, ConstructionError> {
        let operations = Some(capacity.operations);
        let entries = capacity
            .operations
            .checked_mul(3)
            .and_then(|n| n.checked_add(capacity.lineages))
            .and_then(|n| n.checked_add(1));
        Ok(Self {
            flushes: list("the flushes a change makes due", operations)?,
            events: list("the entries a change makes due", entries)?,
            released: list("the operations a change releases", operations)?,
            calls: list("the provider requests a change makes", operations)?,
        })
    }

    /// Empty every list, keeping its room.
    fn clear(&mut self) {
        self.flushes.clear();
        self.events.clear();
        self.released.clear();
        self.calls.clear();
    }
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// The lists a change makes due into: the ones made at construction, or, while those are out,
    /// lists made now at the same sizes.
    pub(super) fn fresh(&mut self) -> Due<E, T, I> {
        if core::mem::replace(&mut self.due_out, true) {
            return tagged(tag::DURABILITY_FRESH_DUE, || Due::reserved(&self.capacity))
                .unwrap_or_default();
        }
        core::mem::take(&mut self.spare)
    }

    /// Take back the lists a change made due into, once they have been acted on, emptied for the
    /// next change. Lists the caller made itself, rather than had from a change, are dropped.
    pub(crate) fn recycle(&mut self, mut due: Due<E, T, I>) {
        if !self.due_out {
            return;
        }
        due.clear();
        self.spare = due;
        self.due_out = false;
    }

    /// Whether the lists a change makes due into are back, as they are between calls.
    #[cfg(test)]
    pub(crate) fn due_home(&self) -> bool {
        !self.due_out
    }
}
