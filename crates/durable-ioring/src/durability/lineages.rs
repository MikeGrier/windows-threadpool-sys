// Copyright (c) 2026 Mike Grier
//! A lineage's lifecycle (DI-D-19, DI-D-30, DI-D-40): minting, enumerating, ending and retiring.
//!
//! An ended lineage is not live from the moment it ends: every query about it answers as for a
//! lineage the instance does not have, and nothing new may join it. Its writes still in flight
//! stay recorded until they complete -- a failure already suspecting one is marked by its end --
//! and so do the writes named to a seal whose commit of their file is out, until it answers
//! (DI-D-98); the lineage is retired once the last has.

use super::{
    DEFAULT_LINEAGE, Due, Durability, Event, Key, LineageState, SealStage, Stage, V, tables, window,
};
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::types::{LineageBusy, LineageInfo};

impl<E: EpochId + 'static, T: Clone, I: Instant, K: StampClock<Instant = I>>
    Durability<E, T, I, K>
{
    /// Mint a lineage, described by `description`.
    pub(crate) fn mint(&mut self, description: Option<String>) -> Key {
        let key = self.next_lineage;
        self.next_lineage += 1;
        if self
            .lineages
            .insert(key, LineageState::new(description))
            .is_err()
        {
            // The I/O core refuses a mint past the lineages declared before it gets here.
            self.inconsistent(
                "a lineage was minted past the lineages declared",
                format_args!("lineage {key}"),
            );
        }
        key
    }

    /// How many lineages are not retired: the live ones, and the ended ones still draining
    /// (DI-D-107).
    pub(crate) fn lineage_count(&self) -> usize {
        self.lineages.len()
    }

    /// Whether `lineage` is live: minted, and neither ended nor retired.
    pub(crate) fn is_live(&self, lineage: Key) -> bool {
        self.live(lineage).is_some()
    }

    /// Each lineage not yet retired, ended ones included, by key, in minting order.
    pub(crate) fn each_lineage(&self, visit: impl FnMut(Key)) {
        self.lineages.keys().copied().for_each(visit);
    }

    /// Each live lineage, in minting order.
    pub(crate) fn lineages(&self, mut visit: impl FnMut(LineageInfo<'_, V<E, I>>)) {
        for (&key, state) in self.lineages.iter().filter(|(_, state)| !state.ended) {
            visit(LineageInfo {
                lineage: self.name(key),
                description: state.description.as_deref(),
                is_default: key == DEFAULT_LINEAGE,
                durable_through: state.durable_through,
                sealed_through: state.sealed_through,
            });
        }
    }

    /// End `lineage` (DI-D-30): abandon every epoch of it not yet durable, and stop answering for
    /// it. Its flushes still out are absorbed when they answer, the writes they were issued for let go
    /// with them; its writes in flight are let go as they complete, and the lineage is retired with
    /// the last. A heal waiting on it no longer
    /// does, and an operation held by a gate on one of its epochs, or a held write of its own, is
    /// never issued (DI-D-43). The lineage must be live and not the default.
    pub(crate) fn end(&mut self, lineage: Key) -> Due<E, T, I> {
        let mut due = self.fresh();
        let Some(state) = self
            .lineages
            .get_mut(&lineage)
            .filter(|state| !state.ended && lineage != DEFAULT_LINEAGE)
        else {
            self.inconsistent(
                "an end of a lineage that is not live, or of the default",
                format_args!("lineage {lineage}"),
            );
            return due;
        };
        let durable = state.durable_through;
        let above = |epoch: &E| durable.is_none_or(|durable| *epoch > durable);
        let sealed = state.sealed_through.filter(above);
        let written = self
            .writes
            .values()
            .filter(|write| write.lineage == lineage)
            .map(|write| write.epoch)
            .chain(tables::of(&self.held_by_epoch, lineage, ..).map(|(key, _)| key.1))
            .filter(above)
            .max();
        let abandoned_through = sealed.max(written);
        self.end_pairs(Some(lineage));
        let state = self
            .lineages
            .get_mut(&lineage)
            .expect("live, checked above");
        state.ended = true;
        state.end_unpopped = true;
        state.leading = None;
        // No failure observed from now on names its writes, the ones it keeps included.
        let now = self.next_failure;
        for write in self.writes.values_mut() {
            if write.lineage == lineage {
                write.until.get_or_insert(now);
            }
        }
        self.let_go_ended(lineage);
        // What stays is a seal with a flush out, to find the writes it let go when it answers.
        let window = &mut self.window;
        let mut lost = None;
        self.seals.retain(|key, seal| {
            if key.0 != lineage {
                return true;
            }
            seal.trailing = None;
            let keep =
                matches!(seal.stage, SealStage::Flushing { unanswered, .. } if unanswered > 0);
            if !keep && let Err(what) = window::unpin(window, seal.pin) {
                lost.get_or_insert(what);
            }
            keep
        });
        if let Some(what) = lost {
            self.inconsistent(what, format_args!("lineage {lineage}, ending"));
        }
        self.emit(
            &mut due,
            Event::LineageEnded {
                lineage,
                abandoned_through,
            },
        );
        self.end_held(lineage, &mut due);
        self.retire_if_drained(lineage);
        self.advance(due)
    }

    /// The consumer popped `lineage`'s `LineageEnded`: it counted until now (DI-D-112), and is
    /// retired if nothing else holds it.
    pub(crate) fn end_popped(&mut self, lineage: Key) {
        match self.lineages.get_mut(&lineage) {
            Some(state) if state.ended && state.end_unpopped => state.end_unpopped = false,
            _ => {
                self.inconsistent(
                    "a LineageEnded was popped for a lineage not ended or already popped",
                    format_args!("lineage {lineage}"),
                );
                return;
            }
        }
        self.retire_if_drained(lineage);
    }

    /// Retire `lineage`, if nothing holds it: no write of it in flight, uncovered or held for a
    /// gate, no operation held for a gate on one of its epochs, and no unresolved failure holding
    /// it. The lineage must be live and not the default.
    pub(crate) fn retire(&mut self, lineage: Key) -> Result<(), LineageBusy<V<E, I>>> {
        if self.live(lineage).is_none() || lineage == DEFAULT_LINEAGE {
            self.inconsistent(
                "a retire of a lineage that is not live, or of the default",
                format_args!("lineage {lineage}"),
            );
            return Ok(());
        }
        let in_flight = tables::of(&self.in_flight_by_epoch, lineage, ..)
            .map(|(_, count)| count)
            .sum();
        let uncovered = self
            .writes
            .values()
            .filter(|write| write.lineage == lineage && write.stage != Stage::InFlight)
            .count();
        let mut failures = 0;
        self.lineage_failures(lineage, |_| failures += 1);
        let held_for_gate = self.held_for_gate(lineage);
        if in_flight > 0 || held_for_gate > 0 || uncovered > 0 || failures > 0 {
            return Err(LineageBusy {
                lineage: self.name(lineage),
                in_flight,
                held_for_gate,
                uncovered,
                failures,
            });
        }
        self.lineages.remove(&lineage);
        Ok(())
    }
}
