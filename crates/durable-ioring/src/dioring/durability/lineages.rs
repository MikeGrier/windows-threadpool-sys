// Copyright (c) 2026 Mike Grier
//! A lineage's lifecycle (DI-D-19, DI-D-30, DI-D-40): minting, enumerating, ending and retiring.
//!
//! An ended lineage is not live from the moment it ends: every query about it answers as for a
//! lineage the instance does not have, and nothing new may join it. Its writes still in flight
//! stay recorded until they complete -- a failure already suspecting one is marked by its end --
//! and the lineage is retired once the last has.

use std::sync::Arc;

use super::{DEFAULT_LINEAGE, Due, Durability, Event, Key, LineageState, Stage, Standing, V};
use crate::contract::EpochId;
use crate::dioring::TimeBase;
use crate::types::{LineageBusy, LineageInfo};

impl<E: EpochId + 'static, T: Clone, K: TimeBase> Durability<E, T, K> {
    /// Mint a lineage, described by `description`.
    pub(crate) fn mint(&mut self, description: Option<String>) -> Key {
        let key = self.next_lineage;
        self.next_lineage += 1;
        self.lineages
            .insert(key, LineageState::new(description.map(Arc::from)));
        key
    }

    /// Whether `lineage` is live: minted, and neither ended nor retired.
    pub(crate) fn is_live(&self, lineage: Key) -> bool {
        self.live(lineage).is_some()
    }

    /// The live lineages, in minting order.
    pub(crate) fn lineages(&self) -> Vec<LineageInfo<V<E>>> {
        self.lineages
            .iter()
            .filter(|(_, state)| !state.ended)
            .map(|(&key, state)| LineageInfo {
                lineage: self.name(key),
                description: state.description.clone(),
                is_default: key == DEFAULT_LINEAGE,
                durable_through: state.durable_through,
                sealed_through: state.sealed_through,
            })
            .collect()
    }

    /// End `lineage` (DI-D-30): abandon every epoch of it not yet durable, and stop answering for
    /// it. Its flushes still out are absorbed when they answer; its writes in flight are let go as
    /// they complete, and the lineage is retired with the last. A heal waiting on it no longer
    /// does. The lineage must be live and not the default.
    pub(crate) fn end(&mut self, lineage: Key) -> Due<E, T> {
        let mut due = Due::default();
        let Some(state) = self
            .lineages
            .get_mut(&lineage)
            .filter(|state| !state.ended && lineage != DEFAULT_LINEAGE)
        else {
            self.inconsistent("an end of a lineage that is not live, or of the default");
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
            .filter(above)
            .max();
        let abandoned_through = sealed.max(written);
        state.ended = true;
        state.seals.clear();
        self.writes
            .retain(|_, write| write.lineage != lineage || write.stage == Stage::InFlight);
        for failure in &mut self.failures {
            if let Standing::Healing { pending } = &mut failure.standing {
                pending.remove(&lineage);
            }
        }
        due.events.push(Event::LineageEnded {
            lineage,
            abandoned_through,
        });
        self.retire_if_drained(lineage);
        self.advance(due)
    }

    /// Retire `lineage`, if nothing holds it: no write of it in flight or uncovered, and no
    /// unresolved failure holding it. The lineage must be live and not the default.
    pub(crate) fn retire(&mut self, lineage: Key) -> Result<(), LineageBusy<V<E>>> {
        let Some(state) = self.live(lineage).filter(|_| lineage != DEFAULT_LINEAGE) else {
            self.inconsistent("a retire of a lineage that is not live, or of the default");
            return Ok(());
        };
        let in_flight = state.in_flight_by_epoch.values().sum();
        let uncovered = self
            .writes
            .values()
            .filter(|write| write.lineage == lineage && write.stage != Stage::InFlight)
            .count();
        let failures: Vec<_> = self
            .failures
            .iter()
            .filter(|failure| failure.holds(lineage).is_some())
            .map(|failure| failure.id)
            .collect();
        if in_flight > 0 || uncovered > 0 || !failures.is_empty() {
            return Err(LineageBusy {
                lineage: self.name(lineage),
                in_flight,
                // Gates are `DI-3.2.5.2`'s.
                held_for_gate: 0,
                uncovered,
                failures,
            });
        }
        self.lineages.remove(&lineage);
        Ok(())
    }
}
