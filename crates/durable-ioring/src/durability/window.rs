// Copyright (c) 2026 Mike Grier
//! The submit window's places (DI-D-100, DI-D-107): one per operation, held until its last holder
//! lets go.
//!
//! An operation has up to three holders. **The operation** holds its place from its push, held for
//! a gate included, until its completion entry is popped. **The write** holds it while it is
//! uncovered: the core keeps its record until a seal covers it, it fails, or its lineage ends.
//! **Each unresolved failure suspecting it** holds it until the failure is healed or abandoned,
//! which is what keeps a failure's suspect set and markings within the places it holds. A place
//! is one, however many holders it has, so the count is of operations and not of holds.
//!
//! Nothing here refuses: the count is what [`Durability::window_places`] reports, and the capacity
//! it is compared with is declared at construction (DI-23.5.2).

use std::collections::HashMap;

use super::{Durability, V};
use crate::alloc_tags::{tag, tagged};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::ids::OpId;
use crate::types::SuspectSet;

#[cfg(test)]
mod tests;

/// Who holds a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Holder {
    /// The operation, until its completion entry is popped.
    Operation,
    /// The write, while it is uncovered.
    Uncovered,
    /// An unresolved failure suspecting the write: one hold for each such failure.
    Suspect,
}

/// What holds one operation's place.
#[derive(Default)]
struct Holds {
    operation: bool,
    uncovered: bool,
    /// How many unresolved failures suspect the write.
    suspected: u32,
}

impl Holds {
    fn is_held(&self) -> bool {
        self.operation || self.uncovered || self.suspected > 0
    }
}

/// The places held, by operation sequence number.
#[derive(Default)]
pub(super) struct Window {
    places: HashMap<u64, Holds>,
}

impl Window {
    /// How many places are held.
    pub(super) fn places(&self) -> usize {
        self.places.len()
    }

    /// `holder` takes a hold on operation `op`'s place, making the place if it is the first.
    ///
    /// # Errors
    ///
    /// What about the hold is inconsistent: the operation or the write already holds it.
    pub(super) fn hold(&mut self, op: u64, holder: Holder) -> Result<(), &'static str> {
        let holds = self.places.entry(op).or_default();
        match holder {
            Holder::Operation if holds.operation => Err("an operation was admitted twice"),
            Holder::Operation => {
                holds.operation = true;
                Ok(())
            }
            Holder::Uncovered if holds.uncovered => Err("a write was recorded uncovered twice"),
            Holder::Uncovered => {
                holds.uncovered = true;
                Ok(())
            }
            Holder::Suspect => {
                holds.suspected += 1;
                Ok(())
            }
        }
    }

    /// `holder` lets go of operation `op`'s place, ending the place if it was the last.
    ///
    /// # Errors
    ///
    /// What about the release is inconsistent: no such place, or `holder` did not hold it.
    pub(super) fn release(&mut self, op: u64, holder: Holder) -> Result<(), &'static str> {
        let Some(holds) = self.places.get_mut(&op) else {
            return Err("a place was released that no one holds");
        };
        let held = match holder {
            Holder::Operation => core::mem::take(&mut holds.operation),
            Holder::Uncovered => core::mem::take(&mut holds.uncovered),
            Holder::Suspect => match holds.suspected.checked_sub(1) {
                Some(rest) => {
                    holds.suspected = rest;
                    true
                }
                None => false,
            },
        };
        if !held {
            return Err("a place was released by a holder that did not hold it");
        }
        if !holds.is_held() {
            self.places.remove(&op);
        }
        Ok(())
    }

    /// The operations whose place `holder` holds, and, for failures, how many hold each.
    #[cfg(test)]
    pub(super) fn held_by(&self, holder: Holder) -> std::collections::BTreeMap<u64, u32> {
        self.places
            .iter()
            .filter_map(|(&op, holds)| {
                let count = match holder {
                    Holder::Operation => u32::from(holds.operation),
                    Holder::Uncovered => u32::from(holds.uncovered),
                    Holder::Suspect => holds.suspected,
                };
                (count > 0).then_some((op, count))
            })
            .collect()
    }
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// How many places of the submit window are held: the operations that have been admitted and
    /// whose completion has not been popped, plus the writes still uncovered, plus the writes an
    /// unresolved failure suspects, each counted once.
    pub(crate) fn window_places(&self) -> usize {
        self.window.places()
    }

    /// Operation `op` was accepted -- pushed, or held for its gate -- and holds its place until its
    /// completion is popped.
    pub(crate) fn admit(&mut self, op: OpId) {
        let held = tagged(tag::DURABILITY_ADMIT_WINDOW, || {
            self.window.hold(op.seq, Holder::Operation)
        });
        self.window_inconsistent(held, op);
    }

    /// Operation `op`'s completion entry was popped: the operation lets go of its place.
    pub(crate) fn popped(&mut self, op: OpId) {
        let released = self.window.release(op.seq, Holder::Operation);
        self.window_inconsistent(released, op);
    }

    /// Write `op` is recorded uncovered, and holds its place for as long.
    pub(super) fn hold_uncovered(&mut self, op: OpId) {
        let held = tagged(tag::DURABILITY_PUSHED_WINDOW, || {
            self.window.hold(op.seq, Holder::Uncovered)
        });
        self.window_inconsistent(held, op);
    }

    /// A failure now suspects every write of `suspect`, and holds each one's place until it is
    /// resolved.
    pub(super) fn hold_suspects(&mut self, suspect: &SuspectSet<V<E, I>>) {
        for write in suspect.writes() {
            let held = tagged(tag::DURABILITY_OBSERVE_WINDOW, || {
                self.window.hold(write.op.seq, Holder::Suspect)
            });
            self.window_inconsistent(held, write.op);
        }
    }

    /// A failure that suspected every write of `suspect` is resolved: it lets go of their places.
    pub(super) fn release_suspects(&mut self, suspect: &SuspectSet<V<E, I>>) {
        for write in suspect.writes() {
            let released = self.window.release(write.op.seq, Holder::Suspect);
            self.window_inconsistent(released, write.op);
        }
    }

    /// Report a hold or a release that was not consistent with what the window held, which is a
    /// defect in the bookkeeping (DI-D-46).
    fn window_inconsistent(&self, result: Result<(), &'static str>, op: OpId) {
        if let Err(what) = result {
            self.inconsistent(what, format_args!("operation {op:?}"));
        }
    }

    /// Whether the places the writes and the failures hold are exactly the uncovered writes, and
    /// each write once for every unresolved failure suspecting it.
    #[cfg(test)]
    pub(super) fn window_agrees(&self) -> bool {
        let uncovered = self.writes.keys().map(|&op| (op, 1)).collect();
        let mut suspected = std::collections::BTreeMap::new();
        for failure in &self.failures {
            for write in failure.suspect.writes() {
                *suspected.entry(write.op.seq).or_insert(0) += 1;
            }
        }
        self.window.held_by(Holder::Uncovered) == uncovered
            && self.window.held_by(Holder::Suspect) == suspected
    }

    /// The operations whose place the operation itself holds.
    #[cfg(test)]
    pub(crate) fn operation_holders(&self) -> std::collections::BTreeSet<u64> {
        self.window.held_by(Holder::Operation).into_keys().collect()
    }
}
