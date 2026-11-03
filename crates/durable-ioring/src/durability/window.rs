// Copyright (c) 2026 Mike Grier
//! The submit window's places (DI-D-100, DI-D-107): one per operation, held until its last holder
//! lets go.
//!
//! An operation has up to five holders. **The operation** holds its place from its push, held for
//! a gate included, until its completion entry is popped. **The write** holds it while it is
//! uncovered: the core keeps its record until a seal covers it, it fails, or its lineage ends.
//! **Each failure suspecting it** holds it until the failure is healed or abandoned and its
//! `Healed` or `Abandoned` entry is popped (DI-D-108), which is what keeps a failure's suspect set
//! and markings, lent until then, within the places it holds. **A
//! recorded seal** holds the place of one write it covers, from when it is made until the mark
//! passes it or its lineage ends (DI-D-110), which is what bounds the seals behind a mark an
//! unresolved failure holds. **An abandoned epoch** above its lineage's mark holds the place of one
//! of its writes until the mark passes it or its lineage ends or is retired (DI-D-109), which is
//! what bounds the epochs a push into is refused for. A place is one, however many holders it has,
//! so the count is of operations and not of holds.
//!
//! Nothing here refuses: the count is what [`Durability::window_places`] reports, and the capacity
//! it is compared with is declared at construction (DI-23.5.2). The table is made at that capacity,
//! so holding a place past it is an inconsistency, which a push's refusal makes unreachable.

use fixed_hash_table::HeapMap;

use super::suspects::suspects;
use super::{Durability, Failure};
use crate::capacity::{ConstructionError, table};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::ids::OpId;

#[cfg(test)]
mod tests;

/// Who holds a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Holder {
    /// The operation, until its completion entry is popped.
    Operation,
    /// The write, while it is uncovered.
    Uncovered,
    /// A failure suspecting the write, until its resolution is popped: one hold for each.
    Suspect,
    /// A recorded seal covering the write, until the mark passes the seal.
    Seal,
    /// An abandoned epoch the write is of, until the mark passes it.
    Abandoned,
}

/// What holds one operation's place.
#[derive(Default)]
struct Holds {
    operation: bool,
    uncovered: bool,
    /// How many unresolved failures suspect the write.
    suspected: u32,
    /// A recorded seal covering the write holds it: a seal holds one write's place, and no two
    /// seals cover the same write.
    sealed: bool,
    /// An abandoned epoch above the mark holds it: one write's place for each such epoch, and a
    /// write is of one epoch.
    abandoned: bool,
}

impl Holds {
    fn is_held(&self) -> bool {
        self.operation || self.uncovered || self.suspected > 0 || self.sealed || self.abandoned
    }
}

/// The places held, by operation sequence number.
pub(super) struct Window {
    places: HeapMap<u64, Holds>,
}

impl Window {
    /// A window of `operations` places, the table's storage allocated now.
    ///
    /// # Errors
    ///
    /// [`ConstructionError::Table`] if no table that large can be made.
    pub(super) fn new(operations: usize) -> Result<Self, ConstructionError> {
        Ok(Self {
            places: table("the places of the submit window", Some(operations))?,
        })
    }

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
        let Ok(holds) = self.places.entry(op).or_default() else {
            return Err("a place was held past the operations declared");
        };
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
            Holder::Seal if holds.sealed => Err("two seals held one write's place"),
            Holder::Seal => {
                holds.sealed = true;
                Ok(())
            }
            Holder::Abandoned if holds.abandoned => {
                Err("two abandoned epochs held one write's place")
            }
            Holder::Abandoned => {
                holds.abandoned = true;
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
            Holder::Seal => core::mem::take(&mut holds.sealed),
            Holder::Abandoned => core::mem::take(&mut holds.abandoned),
        };
        if !held {
            return Err("a place was released by a holder that did not hold it");
        }
        if !holds.is_held() {
            self.places.remove(&op);
        }
        Ok(())
    }

    /// Whether an unresolved failure suspects operation `op`.
    pub(super) fn suspected(&self, op: u64) -> bool {
        self.places
            .get(&op)
            .is_some_and(|holds| holds.suspected > 0)
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
                    Holder::Seal => u32::from(holds.sealed),
                    Holder::Abandoned => u32::from(holds.abandoned),
                };
                (count > 0).then_some((op, count))
            })
            .collect()
    }
}

/// A seal leaves the queue: let go of the place it `pin`s, if it pins one.
///
/// # Errors
///
/// What about the release is inconsistent, as [`Window::release`] says.
pub(super) fn unpin(window: &mut Window, pin: Option<u64>) -> Result<(), &'static str> {
    pin.map_or(Ok(()), |op| window.release(op, Holder::Seal))
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// How many places of the submit window are held: the operations that have been admitted and
    /// whose completion has not been popped, plus the writes still uncovered, plus the writes an
    /// unresolved failure suspects, plus the write each recorded seal holds, plus the write each
    /// abandoned epoch above the mark holds, each counted once.
    pub(crate) fn window_places(&self) -> usize {
        self.window.places()
    }

    /// Operation `op` was accepted -- pushed, or held for its gate -- and holds its place until its
    /// completion is popped.
    pub(crate) fn admit(&mut self, op: OpId) {
        let held = self.window.hold(op.seq, Holder::Operation);
        self.window_inconsistent(held, op);
    }

    /// Operation `op`'s completion entry was popped: the operation lets go of its place.
    pub(crate) fn popped(&mut self, op: OpId) {
        let released = self.window.release(op.seq, Holder::Operation);
        self.window_inconsistent(released, op);
    }

    /// Write `op` is recorded uncovered, and holds its place for as long.
    pub(super) fn hold_uncovered(&mut self, op: OpId) {
        let held = self.window.hold(op.seq, Holder::Uncovered);
        self.window_inconsistent(held, op);
    }

    /// The failure observed `at` in the table holds the place of every write it suspects, until it
    /// is resolved. Every one is uncovered: it was just observed.
    pub(super) fn hold_suspects(&mut self, at: usize) {
        let failure = &self.failures[at];
        let mut lost = None;
        for write in suspects(&self.lists, &self.writes, &self.kept, failure) {
            if let Err(what) = self.window.hold(write.op.seq, Holder::Suspect) {
                lost.get_or_insert((what, write.op));
            }
        }
        if let Some((what, op)) = lost {
            self.window_inconsistent(Err(what), op);
        }
    }

    /// `failure`, out of the table, is resolved: it lets go of its suspects' places, and a kept
    /// write nothing suspects any more is let go.
    pub(super) fn release_suspects(&mut self, failure: &Failure<E, I>) {
        let mut lost = None;
        let uncovered = self.writes.values().filter(|write| {
            failure.names(
                &self.lists,
                write.born,
                write.until,
                write.lineage,
                write.domains,
            )
        });
        for write in uncovered {
            if let Err(what) = self.window.release(write.op.seq, Holder::Suspect) {
                lost.get_or_insert((what, write.op));
            }
        }
        let (window, lists) = (&mut self.window, &self.lists);
        self.kept.release_where(|write| {
            if !failure.names(
                lists,
                write.born,
                Some(write.until),
                write.lineage,
                write.domains,
            ) {
                return false;
            }
            if let Err(what) = window.release(write.op.seq, Holder::Suspect) {
                lost.get_or_insert((what, write.op));
            }
            !window.suspected(write.op.seq)
        });
        if let Some((what, op)) = lost {
            self.window_inconsistent(Err(what), op);
        }
    }

    /// A recorded seal holds the place of write `op`, one it covers, until the mark passes it.
    pub(super) fn hold_for_seal(&mut self, op: u64) {
        if let Err(what) = self.window.hold(op, Holder::Seal) {
            self.inconsistent(what, format_args!("a seal's write, operation {op}"));
        }
    }

    /// Report a hold or a release that was not consistent with what the window held, which is a
    /// defect in the bookkeeping (DI-D-46).
    fn window_inconsistent(&self, result: Result<(), &'static str>, op: OpId) {
        if let Err(what) = result {
            self.inconsistent(what, format_args!("operation {op:?}"));
        }
    }

    /// Whether the places the writes, the failures and the seals hold are exactly the uncovered
    /// writes, each write once for every failure suspecting it not yet forgotten, and the write each
    /// recorded seal pins; whether the kept records are exactly the suspected writes no longer
    /// uncovered (DI-D-108); and whether each failure's rule names exactly the writes observation
    /// listed (DI-D-109).
    #[cfg(test)]
    pub(super) fn window_agrees(&self) -> bool {
        let uncovered = self.writes.keys().map(|&op| (op, 1)).collect();
        let mut suspected = std::collections::BTreeMap::new();
        let mut ruled = true;
        for failure in &self.failures {
            let mut named: std::vec::Vec<u64> =
                suspects(&self.lists, &self.writes, &self.kept, failure)
                    .map(|write| write.op.seq)
                    .collect();
            named.sort_unstable();
            ruled &= named == failure.listed;
            for &op in &named {
                *suspected.entry(op).or_insert(0) += 1;
            }
        }
        let pinned = self
            .seals
            .values()
            .filter_map(|seal| seal.pin)
            .map(|op| (op, 1))
            .collect();
        let kept: std::collections::BTreeSet<u64> = suspected
            .keys()
            .copied()
            .filter(|op| self.writes.get(op).is_none())
            .collect();
        self.window.held_by(Holder::Uncovered) == uncovered
            && self.window.held_by(Holder::Suspect) == suspected
            && self.window.held_by(Holder::Seal) == pinned
            && self.window.held_by(Holder::Abandoned) == self.abandoned_pins()
            && self.kept.ops().collect::<std::collections::BTreeSet<_>>() == kept
            && ruled
    }

    /// The operations whose place the operation itself holds.
    #[cfg(test)]
    pub(crate) fn operation_holders(&self) -> std::collections::BTreeSet<u64> {
        self.window.held_by(Holder::Operation).into_keys().collect()
    }
}
