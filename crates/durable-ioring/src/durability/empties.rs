// Copyright (c) 2026 Mike Grier
//! Seals that cover no writes (DI-D-99): they make no record of their own.
//!
//! A seal whose span holds no write -- none in flight, completed or held for a gate, above the
//! seal point before it and at or below its own -- still takes a sequence number and announces
//! itself, but is kept in fixed space: as the trailing seal of the last recorded seal queued, or,
//! with none queued, in the lineage's leading slot. A later empty seal overwrites the one in its
//! slot, whose pair ends as `Superseded`; the later one's sequence number and `through` satisfy
//! every heal the earlier would have released. Not every mark advance: while a failure holds an
//! epoch between the two, the mark waits below the earlier one's `through` until the failure is
//! resolved, a delay the engineer accepted (DI-D-99).
//!
//! An empty seal is evaluated where it stands in order. It finishes once every write at or below
//! it is out of flight and every seal before it has started, as a recorded seal starts, which is
//! when it releases the heals it is the release of; and the mark passes it only after every seal
//! before it.

use core::iter;
use core::ops::Bound;

use super::{
    Due, Durability, Event, Failure, Key, LineageState, Seal, SealStage, Standing, end_seal, tables,
};
use crate::alloc_tags::{tag, tagged};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::correlation::CorrelationId;
use crate::diagnostics::SealOutcome;

/// An empty seal, in its slot.
pub(super) struct Empty<E> {
    seq: u64,
    through: E,
    /// Every write at or below it is out of flight, and every seal before it has started.
    finished: bool,
    /// Answered on the queue already, by `Blocked`.
    answered: bool,
    /// Its pair's id while the pair is open.
    pair: Option<CorrelationId>,
}

impl<E: Copy> Empty<E> {
    /// An empty seal just made.
    pub(super) fn new(seq: u64, through: E, pair: Option<CorrelationId>) -> Self {
        Self {
            seq,
            through,
            finished: false,
            answered: false,
            pair,
        }
    }

    fn point(&self) -> SealPoint<E> {
        SealPoint {
            seq: self.seq,
            through: self.through,
            finished: self.finished,
        }
    }
}

/// A seal the mark has not passed, recorded or empty, as the queries over seals see it.
#[derive(Clone, Copy, Debug)]
pub(super) struct SealPoint<E> {
    pub(super) seq: u64,
    pub(super) through: E,
    /// Done, for a recorded seal; finished, for an empty one.
    pub(super) finished: bool,
}

/// Where a seal the mark has not passed is kept.
#[derive(Clone, Copy, Debug)]
enum Place {
    Leading,
    Recorded(usize),
    Trailing(usize),
}

impl<E: EpochId> LineageState<E> {
    /// Every seal the mark has not passed, recorded or empty, in the order they were made.
    pub(super) fn seal_points(&self) -> impl Iterator<Item = SealPoint<E>> + Clone + '_ {
        self.leading
            .iter()
            .map(Empty::point)
            .chain(self.seals.iter().flat_map(|seal| {
                iter::once(SealPoint {
                    seq: seal.seq,
                    through: seal.through,
                    finished: matches!(seal.stage, SealStage::Done { .. }),
                })
                .chain(seal.trailing.iter().map(Empty::point))
            }))
    }

    /// Put `empty` in its slot: the trailing seal of the last recorded seal queued, or the
    /// leading slot with none queued. The one it overwrites, if any, is handed back.
    pub(super) fn place(&mut self, empty: Empty<E>) -> Option<Empty<E>> {
        let slot = match self.seals.back_mut() {
            Some(seal) => &mut seal.trailing,
            None => &mut self.leading,
        };
        slot.replace(empty)
    }

    /// The `through` of the seal at `place`, if it is finished successfully and not yet answered.
    fn answerable(&self, place: Place) -> Option<E> {
        match place {
            Place::Leading => self.leading.as_ref().and_then(Empty::answerable),
            Place::Recorded(at) => self.seals.get(at).and_then(|seal| {
                (!seal.answered && matches!(seal.stage, SealStage::Done { succeeded: true }))
                    .then_some(seal.through)
            }),
            Place::Trailing(at) => self
                .seals
                .get(at)
                .and_then(|seal| seal.trailing.as_ref())
                .and_then(Empty::answerable),
        }
    }

    /// Mark the seal at `place` answered, taking its pair.
    fn answer_at(&mut self, place: Place) -> Option<CorrelationId> {
        let (answered, pair) = match place {
            Place::Leading => self
                .leading
                .as_mut()
                .map(|e| (&mut e.answered, &mut e.pair)),
            Place::Recorded(at) => self
                .seals
                .get_mut(at)
                .map(|s| (&mut s.answered, &mut s.pair)),
            Place::Trailing(at) => self
                .seals
                .get_mut(at)
                .and_then(|seal| seal.trailing.as_mut())
                .map(|e| (&mut e.answered, &mut e.pair)),
        }?;
        *answered = true;
        pair.take()
    }

    /// Take every open seal pair, recorded or empty, in the order the seals were made, each with
    /// its seal's `through`.
    pub(super) fn take_pairs(&mut self) -> impl Iterator<Item = (E, CorrelationId)> + '_ {
        let leading = self
            .leading
            .iter_mut()
            .filter_map(|empty| empty.pair.take().map(|pair| (empty.through, pair)));
        let seals = self.seals.iter_mut().flat_map(|seal| {
            let recorded = seal.pair.take().map(|pair| (seal.through, pair));
            let trailing = seal
                .trailing
                .as_mut()
                .and_then(|empty| empty.pair.take().map(|pair| (empty.through, pair)));
            recorded.into_iter().chain(trailing)
        });
        leading.chain(seals)
    }
}

impl<E> Empty<E> {
    fn answerable(&self) -> Option<E>
    where
        E: Copy,
    {
        (self.finished && !self.answered).then_some(self.through)
    }
}

/// Release, in `lineage`, every heal that seal `seq` through `through`, finished successfully, is
/// the release of (DI-D-41, DI-D-46).
pub(super) fn release_heals<E: EpochId + 'static, I: Instant>(
    failures: &mut [Failure<E, I>],
    lineage: Key,
    seq: u64,
    through: E,
) {
    for failure in failures {
        if let Standing::Healing { pending } = &mut failure.standing
            && pending
                .get(&lineage)
                .is_some_and(|release| release.by(seq, through))
        {
            pending.remove(&lineage);
        }
    }
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// The oldest epoch of `lineage` with a write in flight or held for its gate: a seal at or
    /// above it waits.
    pub(super) fn oldest_outstanding(&self, lineage: Key) -> Option<E> {
        let in_flight = tables::lowest(&self.in_flight_by_epoch, lineage);
        let held = tables::lowest(&self.held_by_epoch, lineage);
        match (in_flight, held) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Whether a seal of `lineage` through `through`, made now, would cover no writes: none of the
    /// lineage's writes in flight, completed or held for a gate is above its seal point and at or
    /// below `through`.
    pub(super) fn covers_no_writes(&self, lineage: Key, through: E) -> bool {
        let Some(state) = self.lineages.get(&lineage) else {
            return true;
        };
        let span = (
            state
                .sealed_through
                .map_or(Bound::Unbounded, Bound::Excluded),
            Bound::Included(through),
        );
        !self.writes.values().any(|write| {
            write.lineage == lineage && core::ops::RangeBounds::contains(&span, &write.epoch)
        }) && tables::of(&self.held_by_epoch, lineage, span)
            .next()
            .is_none()
    }

    /// Put an empty seal of `lineage` in its slot, ending the pair of the one it overwrites as
    /// `Superseded`.
    pub(super) fn record_empty(&mut self, lineage: Key, empty: Empty<E>) {
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        if let Some(old) = state.place(empty) {
            end_seal(
                &*self.diagnostics,
                self.instance.number(),
                lineage,
                &old.through,
                old.pair,
                SealOutcome::Superseded,
            );
        }
    }

    /// Finish every empty seal of `lineage` whose writes are all out of flight and whose
    /// predecessors have all started, releasing the heals it is the release of.
    pub(super) fn finish_empties(&mut self, lineage: Key) {
        let oldest = self.oldest_outstanding(lineage);
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        let ready = |through: E| oldest.is_none_or(|oldest| oldest > through);
        let failures = &mut self.failures;
        let mut finish = |empty: &mut Empty<E>| {
            if !empty.finished && ready(empty.through) {
                empty.finished = true;
                release_heals(failures, lineage, empty.seq, empty.through);
            }
        };
        if let Some(empty) = &mut state.leading {
            finish(empty);
        }
        for seal in &mut state.seals {
            if matches!(seal.stage, SealStage::Waiting) {
                // A seal not started has a write at or below it outstanding, so no empty seal
                // after it is ready either: this only stops looking.
                break;
            }
            if let Some(empty) = &mut seal.trailing {
                finish(empty);
            }
        }
    }
}

impl<E: EpochId + 'static, T: Clone, I: Instant, K: crate::clock::StampClock<Instant = I>>
    Durability<E, T, I, K>
{
    /// Advance `lineage`'s high-water mark over its finished seals, recorded and empty, in order,
    /// and answer the seals a failure holds.
    pub(super) fn advance_mark(&mut self, lineage: Key, due: &mut Due<E, T, I>) {
        while let Some(state) = self.lineages.get(&lineage) {
            let (through, empty) = if let Some(empty) = &state.leading {
                if !empty.finished {
                    break;
                }
                (empty.through, true)
            } else if let Some(seal) = state.seals.front() {
                if !matches!(seal.stage, SealStage::Done { .. }) {
                    break;
                }
                (seal.through, false)
            } else {
                break;
            };
            if self.blocking(lineage, through).is_some() {
                break;
            }
            let state = self.lineages.get_mut(&lineage).expect("found above");
            let pair = if empty {
                state.leading.take().expect("found above").pair
            } else {
                let popped: Seal<E> = state.seals.pop_front().expect("found above");
                // What trailed it is now first.
                state.leading = popped.trailing;
                popped.pair
            };
            state.durable_through = Some(through);
            end_seal(
                &*self.diagnostics,
                self.instance.number(),
                lineage,
                &through,
                pair,
                SealOutcome::Durable,
            );
            // Only writes whose flush failed are left at or below a finished seal; once the mark
            // passes them, no later failure can reach back to them (guarantee 3).
            self.let_go(|write| write.lineage == lineage && write.epoch <= through);
            tagged(tag::DURABILITY_ADVANCE_MARK_DURABLE, || {
                due.events.push(Event::Durable { lineage, through });
            });
            self.release_through(lineage, through, due);
        }
        let count = self.lineages.get(&lineage).map_or(0, |s| s.seals.len());
        let places = iter::once(Place::Leading)
            .chain((0..count).flat_map(|at| [Place::Recorded(at), Place::Trailing(at)]));
        for place in places {
            let Some(through) = self
                .lineages
                .get(&lineage)
                .and_then(|state| state.answerable(place))
            else {
                continue;
            };
            let Some(by) = self.blocking(lineage, through) else {
                continue;
            };
            let state = self.lineages.get_mut(&lineage).expect("found above");
            let pair = state.answer_at(place);
            end_seal(
                &*self.diagnostics,
                self.instance.number(),
                lineage,
                &through,
                pair,
                SealOutcome::Blocked,
            );
            tagged(tag::DURABILITY_ADVANCE_MARK_BLOCKED, || {
                due.events.push(Event::Blocked {
                    lineage,
                    through,
                    by,
                });
            });
        }
    }
}
