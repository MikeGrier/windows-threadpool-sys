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
use core::ops::{Bound, RangeBounds};

use super::tables::Seals;
use super::{
    Due, Durability, Event, Failure, Key, LineageState, Seal, SealStage, Standing, end_seal,
    tables, window,
};
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

/// Where a seal the mark has not passed is kept: a recorded seal, and the empty seal trailing it,
/// by the recorded seal's number.
#[derive(Clone, Copy, Debug)]
enum Place {
    Leading,
    Recorded(u64),
    Trailing(u64),
}

impl Place {
    /// The place after this one in `lineage`, in the order the seals were made.
    fn next<E>(self, seals: &Seals<E>, lineage: Key) -> Option<Self> {
        let recorded_after = |after: Option<u64>| {
            tables::seals_of(seals, lineage)
                .map(|(key, _)| key.1)
                .find(|&seq| after.is_none_or(|after| seq > after))
                .map(Self::Recorded)
        };
        match self {
            Self::Leading => recorded_after(None),
            Self::Recorded(seq) => Some(Self::Trailing(seq)),
            Self::Trailing(seq) => recorded_after(Some(seq)),
        }
    }
}

impl<E: EpochId> LineageState<E> {
    /// Every seal the mark has not passed, recorded or empty, in the order they were made.
    pub(super) fn seal_points<'a>(
        &'a self,
        seals: &'a Seals<E>,
        lineage: Key,
    ) -> impl Iterator<Item = SealPoint<E>> + Clone + 'a {
        self.leading
            .iter()
            .map(Empty::point)
            .chain(tables::seals_of(seals, lineage).flat_map(|(_, seal)| {
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
    pub(super) fn place(
        &mut self,
        seals: &mut Seals<E>,
        lineage: Key,
        empty: Empty<E>,
    ) -> Option<Empty<E>> {
        let slot = match tables::seals_of_mut(seals, lineage).next_back() {
            Some((_, seal)) => &mut seal.trailing,
            None => &mut self.leading,
        };
        slot.replace(empty)
    }

    /// The `through` of the seal at `place`, if it is finished successfully and not yet answered.
    fn answerable(&self, seals: &Seals<E>, lineage: Key, place: Place) -> Option<E> {
        match place {
            Place::Leading => self.leading.as_ref().and_then(Empty::answerable),
            Place::Recorded(seq) => seals.get(&(lineage, seq)).and_then(|seal| {
                (!seal.answered && matches!(seal.stage, SealStage::Done { succeeded: true }))
                    .then_some(seal.through)
            }),
            Place::Trailing(seq) => seals
                .get(&(lineage, seq))
                .and_then(|seal| seal.trailing.as_ref())
                .and_then(Empty::answerable),
        }
    }

    /// Mark the seal at `place` answered, taking its pair.
    fn answer_at(
        &mut self,
        seals: &mut Seals<E>,
        lineage: Key,
        place: Place,
    ) -> Option<CorrelationId> {
        let (answered, pair) = match place {
            Place::Leading => self
                .leading
                .as_mut()
                .map(|e| (&mut e.answered, &mut e.pair)),
            Place::Recorded(seq) => seals
                .get_mut(&(lineage, seq))
                .map(|s| (&mut s.answered, &mut s.pair)),
            Place::Trailing(seq) => seals
                .get_mut(&(lineage, seq))
                .and_then(|seal| seal.trailing.as_mut())
                .map(|e| (&mut e.answered, &mut e.pair)),
        }?;
        *answered = true;
        pair.take()
    }

    /// Take every open seal pair, recorded or empty, in the order the seals were made, each with
    /// its seal's `through`.
    pub(super) fn take_pairs<'a>(
        &'a mut self,
        seals: &'a mut Seals<E>,
        lineage: Key,
    ) -> impl Iterator<Item = (E, CorrelationId)> + 'a {
        let leading = self
            .leading
            .iter_mut()
            .filter_map(|empty| empty.pair.take().map(|pair| (empty.through, pair)));
        let seals = tables::seals_of_mut(seals, lineage).flat_map(|(_, seal)| {
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

    /// The write whose place a seal of `lineage` through `through`, made now, holds until the mark
    /// passes it (DI-D-110): the earliest pushed of the lineage's writes in flight, completed or
    /// held for a gate above its seal point and at or below `through`. `None` when there is none,
    /// which is a seal that covers no writes and makes no record (DI-D-99).
    pub(super) fn pinned_by(&self, lineage: Key, through: E) -> Option<u64> {
        let state = self.lineages.get(&lineage)?;
        let span = (
            state
                .sealed_through
                .map_or(Bound::Unbounded, Bound::Excluded),
            Bound::Included(through),
        );
        let covers = |at: Key, epoch: &E| at == lineage && span.contains(epoch);
        let written = self
            .writes
            .iter()
            .find(|(_, write)| covers(write.lineage, &write.epoch))
            .map(|(&seq, _)| seq);
        let held = self
            .held
            .iter()
            .find(|(_, held)| {
                held.write
                    .as_ref()
                    .is_some_and(|write| covers(write.lineage, &write.epoch))
            })
            .map(|(&seq, _)| seq);
        match (written, held) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Put an empty seal of `lineage` in its slot, ending the pair of the one it overwrites as
    /// `Superseded`. The later seal takes over the earlier one's request, so one already answered
    /// `Blocked` has that entry withdrawn (DI-D-112).
    pub(super) fn record_empty(&mut self, lineage: Key, empty: Empty<E>, due: &mut Due<E, T, I>) {
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        if let Some(old) = state.place(&mut self.seals, lineage, empty) {
            end_seal(
                &*self.diagnostics,
                self.instance.number(),
                lineage,
                &old.through,
                old.pair,
                SealOutcome::Superseded,
            );
            if old.answered {
                self.emit(
                    due,
                    Event::Superseded {
                        lineage,
                        through: old.through,
                    },
                );
            }
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
        for (_, seal) in tables::seals_of_mut(&mut self.seals, lineage) {
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
            let (through, recorded) = if let Some(empty) = &state.leading {
                if !empty.finished {
                    break;
                }
                (empty.through, None)
            } else if let Some((_, seal)) = tables::seals_of(&self.seals, lineage).next() {
                if !matches!(seal.stage, SealStage::Done { .. }) {
                    break;
                }
                (seal.through, Some(seal.seq))
            } else {
                break;
            };
            if self.blocking(lineage, through).is_some() {
                break;
            }
            let state = self.lineages.get_mut(&lineage).expect("found above");
            let mut unpinned = Ok(());
            let pair = if let Some(seq) = recorded {
                let popped: Seal<E> = self.seals.remove(&(lineage, seq)).expect("found above");
                // What trailed it is now first.
                state.leading = popped.trailing;
                unpinned = window::unpin(&mut self.window, popped.pin);
                popped.pair
            } else {
                state.leading.take().expect("found above").pair
            };
            state.durable_through = Some(through);
            if let Err(what) = unpinned {
                self.inconsistent(what, format_args!("lineage {lineage}, through {through:?}"));
            }
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
            self.emit(due, Event::Durable { lineage, through });
            self.release_through(lineage, through, due);
        }
        let mut next = Some(Place::Leading);
        while let Some(place) = next {
            next = place.next(&self.seals, lineage);
            let Some(through) = self
                .lineages
                .get(&lineage)
                .and_then(|state| state.answerable(&self.seals, lineage, place))
            else {
                continue;
            };
            let Some(by) = self.blocking(lineage, through) else {
                continue;
            };
            let state = self.lineages.get_mut(&lineage).expect("found above");
            let pair = state.answer_at(&mut self.seals, lineage, place);
            end_seal(
                &*self.diagnostics,
                self.instance.number(),
                lineage,
                &through,
                pair,
                SealOutcome::Blocked,
            );
            self.emit(
                due,
                Event::Blocked {
                    lineage,
                    through,
                    by,
                },
            );
        }
    }
}
