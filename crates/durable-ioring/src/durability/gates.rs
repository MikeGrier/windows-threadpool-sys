// Copyright (c) 2026 Mike Grier
//! Gates (DI-D-23, DI-D-43): operations held until an epoch is durable, released when their gate's
//! lineage reaches it, ended as never issued when an epoch they need is abandoned, and the cycle
//! search that refuses a push or a seal that would leave two of them waiting on each other.
//!
//! **The dependency a gate creates.** A held write tagged `L:a`, gated on `M:b`, cannot complete
//! until `M` reaches `b`; and `L` cannot reach any epoch whose seal must wait for `a`'s writes
//! until it does. So "`L` reaching `y`" needs every held write of `L` up to the **extent** of `y`:
//! the furthest epoch whose writes `L` must complete before its mark passes `y`. That is `y` itself,
//! raised to the first seal at or above `y`, and -- while a healing failure holds an epoch of `L` at
//! or below `y` -- to the seal its heal waits for: the first made after the heal that reaches the
//! highest epoch `L` had been pushed then (DI-D-41, DI-D-46). A seal not yet made is no dependency:
//! the seal that will cover `y` is checked when it is made, which is why a seal can be refused.
//!
//! **The search, flattened** (the engineer's direction, 2026-10-08: the success case fast, the
//! detailed answer only once there is a cycle to describe). A lineage's needs are monotone in the
//! epoch it must reach, so the search keeps one bound per lineage -- how far its held writes are
//! already needed -- and only ever raises it. Raising a bound scans just the newly covered range of
//! that lineage's held writes, sorted by epoch, and queues their gates. Each lineage's writes are
//! scanned once overall, whatever order the gates arrive in, so the answer does not depend on the
//! order of exploration. It records nothing about how a lineage was reached; when the answer is a
//! cycle, [`Durability::trace`] repeats the search with a parent per step to name it. A push with
//! no gate, and a seal over no held write, never search at all.

use core::ops::{Bound, RangeBounds};

use super::kept::Kept;
use super::suspects::suspects;
use super::{
    Accepted, Due, Durability, Event, Failure, Gate, HeldOp, Key, Standing, State, Write, tables,
};
use crate::alloc_tags::{tag, tagged};
use crate::capacity::within;
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::domain_lists::DomainLists;
use crate::ids::OpId;
use crate::ordered::Ordered;

/// Where a gate stands when an operation names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GateState {
    /// A lineage the instance cannot answer for: another instance's, ended, or retired.
    Unknown,
    /// Abandoned: it can never be durable.
    Abandoned,
    /// Durable already: the operation need not wait.
    Durable,
    /// Not yet durable: the operation is held.
    Waiting,
}

/// One step of a traced cycle: a held write, and the gate it waits for.
pub(super) struct Step<E> {
    write: Gate<E>,
    gate: Gate<E>,
    /// The step whose gate needed this write; the first step has none.
    parent: Option<usize>,
}

/// The epochs above `lower` and at or below `upper`.
fn span<E>(lower: Bound<E>, upper: E) -> (Bound<E>, Bound<E>) {
    (lower, Bound::Included(upper))
}

/// Raise `lineage`'s bound to `extent`, returning the bound below the newly covered range, or
/// `None` if the bound already covers it. `Err` is a lineage past the room `bounds` was made with,
/// which is one for each lineage declared.
fn raise<E: EpochId>(
    bounds: &mut Vec<(Key, E)>,
    lineage: Key,
    extent: E,
) -> Result<Option<Bound<E>>, Key> {
    match bounds.iter_mut().find(|(key, _)| *key == lineage) {
        Some((_, bound)) if *bound >= extent => Ok(None),
        Some((_, bound)) => Ok(Some(Bound::Excluded(core::mem::replace(bound, extent)))),
        None => match within(bounds, (lineage, extent)) {
            Ok(()) => Ok(Some(Bound::Unbounded)),
            Err(_) => Err(lineage),
        },
    }
}

impl<E: EpochId + 'static, T: Clone, I: Instant, K: StampClock<Instant = I>>
    Durability<E, T, I, K>
{
    /// Where `gate` stands.
    pub(crate) fn gate_state(&self, gate: Gate<E>) -> GateState {
        match self.state(gate.lineage, gate.epoch) {
            None => GateState::Unknown,
            Some(State::Abandoned) => GateState::Abandoned,
            Some(State::Durable) => GateState::Durable,
            Some(_) => GateState::Waiting,
        }
    }

    /// The cycle a write tagged `write`, gated on `gate`, would close, if any (DI-D-23): whether
    /// `gate`'s lineage reaching it needs, transitively, `write`'s lineage to complete writes at or
    /// above `write`'s epoch. A gate on the write's own lineage at or above its epoch is the
    /// one-step case.
    pub(crate) fn write_cycle(&mut self, write: Gate<E>, gate: Gate<E>) -> Option<Vec<Gate<E>>> {
        let mut work = core::mem::take(&mut self.scratch.work);
        let mut bounds = core::mem::take(&mut self.scratch.bounds);
        let found = self.seed(&mut work, gate) && self.needs(&mut work, &mut bounds, write, None);
        bounds.clear();
        let cycle = found.then(|| self.trace(&mut bounds, write, gate, None));
        work.clear();
        bounds.clear();
        self.scratch.work = work;
        self.scratch.bounds = bounds;
        cycle
    }

    /// The cycle sealing `lineage` through `through` would close, if any (DI-D-43). The seal only
    /// raises extents in `lineage`, and only up to `through`, so a cycle it closes passes through a
    /// held write of `lineage` at or below `through`. One search from all of them at once is the
    /// cheap test: if it never needs the lowest of them, none is on a cycle. Only if it does is each
    /// tested alone, in epoch order.
    pub(super) fn seal_cycle(&mut self, lineage: Key, through: E) -> Option<Vec<Gate<E>>> {
        self.live(lineage)?;
        let extra = Some(Gate {
            lineage,
            epoch: through,
        });
        let mut work = core::mem::take(&mut self.scratch.work);
        let mut bounds = core::mem::take(&mut self.scratch.bounds);
        let found = self.sealing_needs(&mut work, &mut bounds, lineage, through, extra);
        bounds.clear();
        let cycle = found.map(|(write, gate)| self.trace(&mut bounds, write, gate, extra));
        work.clear();
        bounds.clear();
        self.scratch.work = work;
        self.scratch.bounds = bounds;
        cycle
    }

    /// The held write of `lineage` at or below `through`, and its gate, that sealing it through
    /// `through` would put on a cycle, if any. `work` and `bounds` are the search's, empty.
    fn sealing_needs(
        &self,
        work: &mut Vec<Gate<E>>,
        bounds: &mut Vec<(Key, E)>,
        lineage: Key,
        through: E,
        extra: Option<Gate<E>>,
    ) -> Option<(Gate<E>, Gate<E>)> {
        let covered = tables::of(&self.gated, lineage, ..=through);
        let (&(_, lowest, _), _) = covered.clone().next()?;
        let lowest = Gate {
            lineage,
            epoch: lowest,
        };
        let seeded = covered.clone().all(|(_, &gate)| self.seed(work, gate));
        if !seeded || !self.needs(work, bounds, lowest, extra) {
            return None;
        }
        covered
            .map(|(&(_, epoch, _), &gate)| (Gate { lineage, epoch }, gate))
            .find(|&(write, gate)| {
                work.clear();
                bounds.clear();
                self.seed(work, gate) && self.needs(work, bounds, write, extra)
            })
    }

    /// Put `gate` on a search's `work`, reporting a gate past its room as an inconsistency.
    /// Whether it fitted.
    fn seed(&self, work: &mut Vec<Gate<E>>, gate: Gate<E>) -> bool {
        let fits = within(work, gate).is_ok();
        if !fits {
            self.inconsistent(
                "a cycle search visited more gates than the operations declared",
                format_args!("gate {gate:?}"),
            );
        }
        fits
    }

    /// Put `step` on a trace's `steps`, reporting a step past their room as an inconsistency.
    /// Whether it fitted.
    fn step(&self, steps: &mut Vec<Step<E>>, step: Step<E>) -> bool {
        let (write, gate) = (step.write, step.gate);
        let fits = within(steps, step).is_ok();
        if !fits {
            self.inconsistent(
                "a traced cycle took more steps than the operations declared",
                format_args!("write {write:?}, gated on {gate:?}"),
            );
        }
        fits
    }

    /// How far `target`'s lineage must complete its writes for its mark to reach `target`'s epoch,
    /// or `None` if the mark already has, or the lineage is not live. `extra` is a seal being
    /// considered, as though it had been made.
    fn extent(&self, target: Gate<E>, extra: Option<Gate<E>>) -> Option<E> {
        let state = self.live(target.lineage)?;
        let mark = state.durable_through;
        if mark.is_some_and(|mark| mark >= target.epoch) {
            return None;
        }
        let considered = extra
            .filter(|seal| seal.lineage == target.lineage)
            .map(|seal| (state.next_seal, seal.epoch));
        let seals = state
            .seal_points(&self.seals, target.lineage)
            .map(|seal| (seal.seq, seal.through))
            .chain(considered);
        let mut extent = target.epoch;
        if let Some((_, through)) = seals.clone().find(|&(_, through)| through >= target.epoch) {
            extent = extent.max(through);
        }
        let lower = mark.map_or(Bound::Unbounded, Bound::Excluded);
        for failure in &self.failures {
            let Standing::Healing { pending } = &failure.standing else {
                continue;
            };
            let Some(release) = pending.get(&target.lineage) else {
                continue;
            };
            let holds = suspects(&self.lists, &self.writes, &self.kept, failure).any(|write| {
                write.lineage == target.lineage
                    && (lower, Bound::Included(target.epoch)).contains(&write.epoch)
            });
            if holds
                && let Some((_, through)) = seals
                    .clone()
                    .find(|&(seq, through)| release.by(seq, through))
            {
                extent = extent.max(through);
            }
        }
        Some(extent)
    }

    /// Whether reaching every gate on `work`, its seeds, needs, transitively, `goal`'s lineage to
    /// complete its writes at or above `goal`'s epoch. `bounds` starts empty; both are the
    /// search's own, and are left as it left them.
    fn needs(
        &self,
        work: &mut Vec<Gate<E>>,
        bounds: &mut Vec<(Key, E)>,
        goal: Gate<E>,
        extra: Option<Gate<E>>,
    ) -> bool {
        while let Some(gate) = work.pop() {
            let Some(extent) = self.extent(gate, extra) else {
                continue;
            };
            if gate.lineage == goal.lineage && extent >= goal.epoch {
                return true;
            }
            let lower = match raise(bounds, gate.lineage, extent) {
                Ok(Some(lower)) => lower,
                Ok(None) => continue,
                Err(lineage) => {
                    self.unbounded(lineage);
                    return false;
                }
            };
            let mut gated = tables::of(&self.gated, gate.lineage, span(lower, extent));
            if !gated.all(|(_, &gate)| self.seed(work, gate)) {
                return false;
            }
        }
        false
    }

    /// A search reached more lineages than were declared: an inconsistency.
    fn unbounded(&self, lineage: Key) {
        self.inconsistent(
            "a cycle search reached more lineages than were declared",
            format_args!("lineage {lineage}"),
        );
    }

    /// The cycle through a held write `start`, gated on `gate`, that [`Self::needs`] found: each
    /// held write's epoch followed by its gate, starting at `start`, and ending at the gate whose
    /// reaching needs `start` again. The same search, breadth first, with a parent per step, so the
    /// cycle named is a shortest one.
    fn trace(
        &mut self,
        bounds: &mut Vec<(Key, E)>,
        start: Gate<E>,
        gate: Gate<E>,
        extra: Option<Gate<E>>,
    ) -> Vec<Gate<E>> {
        let mut steps = core::mem::take(&mut self.scratch.steps);
        let cycle = self.trace_in(&mut steps, bounds, start, gate, extra);
        steps.clear();
        self.scratch.steps = steps;
        cycle
    }

    /// [`trace`](Self::trace), into its `steps` and `bounds`, both empty.
    fn trace_in(
        &self,
        steps: &mut Vec<Step<E>>,
        bounds: &mut Vec<(Key, E)>,
        start: Gate<E>,
        gate: Gate<E>,
        extra: Option<Gate<E>>,
    ) -> Vec<Gate<E>> {
        let first = Step {
            write: start,
            gate,
            parent: None,
        };
        if !self.step(steps, first) {
            return tagged(tag::GATES_TRACE_UNFOUND, || vec![start, gate]);
        }
        let mut next = 0;
        while next < steps.len() {
            let index = next;
            next += 1;
            let gate = steps[index].gate;
            let Some(extent) = self.extent(gate, extra) else {
                continue;
            };
            if gate.lineage == start.lineage && extent >= start.epoch {
                let mut cycle = Vec::new();
                let mut at = Some(index);
                while let Some(step) = at {
                    tagged(tag::GATES_TRACE_CYCLE_GATE, || cycle.push(steps[step].gate));
                    tagged(tag::GATES_TRACE_CYCLE_WRITE, || {
                        cycle.push(steps[step].write)
                    });
                    at = steps[step].parent;
                }
                cycle.reverse();
                return cycle;
            }
            let lower = match raise(bounds, gate.lineage, extent) {
                Ok(Some(lower)) => lower,
                Ok(None) => continue,
                Err(lineage) => {
                    self.unbounded(lineage);
                    break;
                }
            };
            for (&(_, epoch, _), &next_gate) in
                tables::of(&self.gated, gate.lineage, span(lower, extent))
            {
                let step = Step {
                    write: Gate {
                        lineage: gate.lineage,
                        epoch,
                    },
                    gate: next_gate,
                    parent: Some(index),
                };
                if !self.step(steps, step) {
                    return tagged(tag::GATES_TRACE_UNFOUND, || vec![start, gate]);
                }
            }
        }
        // `needs` found the cycle, and this is the same search.
        tagged(tag::GATES_TRACE_UNFOUND, || vec![start, gate])
    }

    /// Hold operation `op` until `gate` is durable: a write, with what it will be recorded as once
    /// issued, or a read. The caller has found the gate waiting and, for a write, no cycle.
    pub(crate) fn hold(&mut self, op: OpId, write: Option<Accepted<E, T>>, gate: Gate<E>) {
        let lineage_live = write
            .as_ref()
            .is_none_or(|write| self.live(write.lineage).is_some());
        if !lineage_live || self.live(gate.lineage).is_none() {
            let lineage = write.as_ref().map(|write| (write.lineage, write.epoch));
            self.inconsistent(
                "an operation was held on a lineage that is not live",
                format_args!("operation {op:?}, written at {lineage:?}, gated on {gate:?}"),
            );
            return;
        }
        if let Some(write) = &write {
            let state = self.lineages.get_mut(&write.lineage).expect("live");
            state.pushed_through = state.pushed_through.max(Some(write.epoch));
            let gated = self
                .gated
                .insert((write.lineage, write.epoch, op.seq), gate)
                .is_ok();
            let counted =
                tables::count_up(&mut self.held_by_epoch, write.lineage, write.epoch).is_ok();
            if !gated || !counted {
                self.inconsistent(
                    "a held write was recorded past the operations declared",
                    format_args!("write {op:?}, gated on {gate:?}"),
                );
                return;
            }
        }
        let held = HeldOp {
            op,
            write,
            gate,
            ready: false,
        };
        if self.held.insert(op.seq, held).is_err() {
            self.inconsistent(
                "an operation was held past the operations declared",
                format_args!("operation {op:?}, gated on {gate:?}"),
            );
        }
    }

    /// A released operation was pushed to the ring: a write is recorded as in flight from now.
    pub(crate) fn issued(&mut self, op: OpId) {
        match self.held.remove(&op.seq) {
            Some(HeldOp {
                write, ready: true, ..
            }) => {
                if let Some(write) = write {
                    self.unhold(write.lineage, write.epoch);
                    self.in_flight(write, true);
                }
            }
            Some(held) => {
                let gate = held.gate;
                // Its slot was just let go, so it has room.
                let _ = self.held.insert(op.seq, held);
                self.inconsistent(
                    "an operation was issued before its gate was durable",
                    format_args!("operation {op:?}, gated on {gate:?}"),
                );
            }
            None => self.inconsistent(
                "an operation was issued that was never held",
                format_args!("operation {op:?}"),
            ),
        }
    }

    /// Release every operation waiting for an epoch of `lineage` at or below `through`, which its
    /// mark has just reached, in push order. Abandoned epochs never reach this: abandoning one ends
    /// what waits for it at once.
    pub(super) fn release_through(&mut self, lineage: Key, through: E, due: &mut Due<E, T, I>) {
        // The held operations are kept in push order, so they are released in it.
        let mut unlisted = None;
        for held in self.held.values_mut() {
            if held.ready || held.gate.lineage != lineage || held.gate.epoch > through {
                continue;
            }
            held.ready = true;
            if let Some(write) = &held.write {
                self.gated
                    .remove(&(write.lineage, write.epoch, held.op.seq));
            }
            if within(&mut due.released, held.op).is_err() {
                unlisted.get_or_insert(held.op);
            }
        }
        if let Some(op) = unlisted {
            self.inconsistent(
                "a change released more operations than the operations declared",
                format_args!("operation {op:?}, lineage {lineage}, through {through:?}"),
            );
        }
    }

    /// End as never issued every held operation an abandoned failure stopped (DI-D-43): one whose
    /// gate it abandoned, and a write whose own epoch it abandoned, which no write may join
    /// (DI-D-35). The gate is named when both were.
    pub(super) fn abandon_held(&mut self, failure: &Failure<E, I>, due: &mut Due<E, T, I>) {
        self.stop_held(
            |lists, writes, kept, held| {
                let hit = |at: Gate<E>| {
                    suspects(lists, writes, kept, failure)
                        .any(|write| write.lineage == at.lineage && write.epoch == at.epoch)
                };
                if !held.ready && hit(held.gate) {
                    return Some(held.gate);
                }
                held.write
                    .as_ref()
                    .map(|write| Gate {
                        lineage: write.lineage,
                        epoch: write.epoch,
                    })
                    .filter(|&own| hit(own))
            },
            due,
        );
    }

    /// End as never issued every held operation an ended lineage stopped (DI-D-30): a write of
    /// it, naming its own epoch, and an operation still waiting for one of its epochs, naming the
    /// gate.
    pub(super) fn end_held(&mut self, lineage: Key, due: &mut Due<E, T, I>) {
        self.stop_held(
            |_, _, _, held| match &held.write {
                Some(write) if write.lineage == lineage => Some(Gate {
                    lineage,
                    epoch: write.epoch,
                }),
                _ => (!held.ready && held.gate.lineage == lineage).then_some(held.gate),
            },
            due,
        );
    }

    /// How many operations hold `lineage` for a gate: its own held writes, and operations still
    /// waiting for one of its epochs.
    pub(super) fn held_for_gate(&self, lineage: Key) -> usize {
        self.held
            .values()
            .filter(|held| {
                held.write
                    .as_ref()
                    .is_some_and(|write| write.lineage == lineage)
                    || (!held.ready && held.gate.lineage == lineage)
            })
            .count()
    }

    /// End as never issued, in push order, every held operation `stopped` names an abandoned epoch
    /// for, given the uncovered and the kept writes to read.
    fn stop_held(
        &mut self,
        stopped: impl Fn(
            &DomainLists,
            &Ordered<u64, Write<E, T>>,
            &Kept<E, I>,
            &HeldOp<E, T>,
        ) -> Option<Gate<E>>,
        due: &mut Due<E, T, I>,
    ) {
        let (lists, writes, kept) = (&self.lists, &self.writes, &self.kept);
        let (gated, held_by_epoch) = (&mut self.gated, &mut self.held_by_epoch);
        self.held.take_where(
            |_, held| stopped(lists, writes, kept, held).is_some(),
            |&seq, held| {
                let Some(abandoned) = stopped(lists, writes, kept, &held) else {
                    return;
                };
                if let Some(write) = &held.write {
                    gated.remove(&(write.lineage, write.epoch, seq));
                    tables::count_down(held_by_epoch, write.lineage, write.epoch);
                }
                tagged(tag::GATES_STOP_HELD_EVENT, || {
                    due.events.push(Event::NeverIssued {
                        op: held.op,
                        abandoned,
                    });
                });
            },
        );
    }

    /// One held write of `lineage` at `epoch` is no longer held.
    fn unhold(&mut self, lineage: Key, epoch: E) {
        tables::count_down(&mut self.held_by_epoch, lineage, epoch);
    }
}
