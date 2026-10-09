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

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ops::Bound;

use super::{Accepted, Due, Durability, Event, Gate, HeldOp, Key, Standing, State};
use crate::contract::EpochId;
use crate::dioring::TimeBase;
use crate::ids::OpId;

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
struct Step<E> {
    write: Gate<E>,
    gate: Gate<E>,
    /// The step whose gate needed this write; the first step has none.
    parent: Option<usize>,
}

/// A range of a lineage's held writes, keyed by epoch and push order.
type Span<E> = (Bound<(E, u64)>, Bound<(E, u64)>);

/// The range of a lineage's held writes with epochs above `lower` and at or below `upper`.
fn span<E: EpochId>(lower: Bound<E>, upper: E) -> Span<E> {
    let lower = match lower {
        Bound::Unbounded => Bound::Unbounded,
        Bound::Excluded(epoch) => Bound::Excluded((epoch, u64::MAX)),
        Bound::Included(epoch) => Bound::Included((epoch, 0)),
    };
    (lower, Bound::Included((upper, u64::MAX)))
}

/// Raise `lineage`'s bound to `extent`, returning the bound below the newly covered range, or
/// `None` if the bound already covers it.
fn raise<E: EpochId>(bounds: &mut Vec<(Key, E)>, lineage: Key, extent: E) -> Option<Bound<E>> {
    match bounds.iter_mut().find(|(key, _)| *key == lineage) {
        Some((_, bound)) if *bound >= extent => None,
        Some((_, bound)) => Some(Bound::Excluded(std::mem::replace(bound, extent))),
        None => {
            bounds.push((lineage, extent));
            Some(Bound::Unbounded)
        }
    }
}

impl<E: EpochId + 'static, T: Clone, K: TimeBase> Durability<E, T, K> {
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
    pub(crate) fn write_cycle(&self, write: Gate<E>, gate: Gate<E>) -> Option<Vec<Gate<E>>> {
        self.needs(vec![gate], write, None)
            .then(|| self.trace(write, gate, None))
    }

    /// The cycle sealing `lineage` through `through` would close, if any (DI-D-43). The seal only
    /// raises extents in `lineage`, and only up to `through`, so a cycle it closes passes through a
    /// held write of `lineage` at or below `through`. One search from all of them at once is the
    /// cheap test: if it never needs the lowest of them, none is on a cycle. Only if it does is each
    /// tested alone, in epoch order.
    pub(super) fn seal_cycle(&self, lineage: Key, through: E) -> Option<Vec<Gate<E>>> {
        let state = self.live(lineage)?;
        let covered = state.gated.range(..=(through, u64::MAX));
        let (&(lowest, _), _) = covered.clone().next()?;
        let extra = Some(Gate {
            lineage,
            epoch: through,
        });
        let lowest = Gate {
            lineage,
            epoch: lowest,
        };
        if !self.needs(
            covered.clone().map(|(_, gate)| *gate).collect(),
            lowest,
            extra,
        ) {
            return None;
        }
        covered
            .map(|(&(epoch, _), &gate)| (Gate { lineage, epoch }, gate))
            .find(|&(write, gate)| self.needs(vec![gate], write, extra))
            .map(|(write, gate)| self.trace(write, gate, extra))
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
            .seals
            .iter()
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
            let holds = failure.epochs.get(&target.lineage).is_some_and(|epochs| {
                epochs
                    .range((lower, Bound::Included(target.epoch)))
                    .next()
                    .is_some()
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

    /// Whether reaching every gate in `seeds` needs, transitively, `goal`'s lineage to complete its
    /// writes at or above `goal`'s epoch.
    fn needs(&self, seeds: Vec<Gate<E>>, goal: Gate<E>, extra: Option<Gate<E>>) -> bool {
        let mut bounds = Vec::new();
        let mut work = seeds;
        while let Some(gate) = work.pop() {
            let Some(extent) = self.extent(gate, extra) else {
                continue;
            };
            if gate.lineage == goal.lineage && extent >= goal.epoch {
                return true;
            }
            let Some(lower) = raise(&mut bounds, gate.lineage, extent) else {
                continue;
            };
            let gated = &self.lineages[&gate.lineage].gated;
            work.extend(gated.range(span(lower, extent)).map(|(_, gate)| *gate));
        }
        false
    }

    /// The cycle through a held write `start`, gated on `gate`, that [`Self::needs`] found: each
    /// held write's epoch followed by its gate, starting at `start`, and ending at the gate whose
    /// reaching needs `start` again. The same search, breadth first, with a parent per step, so the
    /// cycle named is a shortest one.
    fn trace(&self, start: Gate<E>, gate: Gate<E>, extra: Option<Gate<E>>) -> Vec<Gate<E>> {
        let mut steps = vec![Step {
            write: start,
            gate,
            parent: None,
        }];
        let mut bounds = Vec::new();
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
                    cycle.push(steps[step].gate);
                    cycle.push(steps[step].write);
                    at = steps[step].parent;
                }
                cycle.reverse();
                return cycle;
            }
            let Some(lower) = raise(&mut bounds, gate.lineage, extent) else {
                continue;
            };
            let gated = &self.lineages[&gate.lineage].gated;
            for (&(epoch, _), &next_gate) in gated.range(span(lower, extent)) {
                steps.push(Step {
                    write: Gate {
                        lineage: gate.lineage,
                        epoch,
                    },
                    gate: next_gate,
                    parent: Some(index),
                });
            }
        }
        // `needs` found the cycle, and this is the same search.
        vec![start, gate]
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
            state.gated.insert((write.epoch, op.seq), gate);
            *state.held_by_epoch.entry(write.epoch).or_insert(0) += 1;
            state.pushed_through = state.pushed_through.max(Some(write.epoch));
        }
        let state = self.lineages.get_mut(&gate.lineage).expect("live");
        state.waiting.insert((gate.epoch, op.seq));
        self.held.insert(
            op.seq,
            HeldOp {
                op,
                write,
                gate,
                ready: false,
            },
        );
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
                self.held.insert(op.seq, held);
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
    pub(super) fn release_through(&mut self, lineage: Key, through: E, due: &mut Due<E, T>) {
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        if state
            .waiting
            .first()
            .is_none_or(|&(epoch, _)| epoch > through)
        {
            return;
        }
        let rest = state.waiting.split_off(&(through, u64::MAX));
        let mut released: Vec<u64> = std::mem::replace(&mut state.waiting, rest)
            .into_iter()
            .map(|(_, seq)| seq)
            .collect();
        released.sort_unstable();
        for seq in released {
            let Some(held) = self.held.get_mut(&seq) else {
                continue;
            };
            held.ready = true;
            if let Some(write) = &held.write
                && let Some(state) = self.lineages.get_mut(&write.lineage)
            {
                state.gated.remove(&(write.epoch, seq));
            }
            due.released.push(held.op);
        }
    }

    /// End as never issued every held operation an abandoned failure stopped (DI-D-43): one whose
    /// gate it abandoned, and a write whose own epoch it abandoned, which no write may join
    /// (DI-D-35). The gate is named when both were.
    pub(super) fn abandon_held(
        &mut self,
        epochs: &BTreeMap<Key, BTreeSet<E>>,
        due: &mut Due<E, T>,
    ) {
        let hit = |at: Gate<E>| {
            epochs
                .get(&at.lineage)
                .is_some_and(|epochs| epochs.contains(&at.epoch))
        };
        self.stop_held(
            |held| {
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
    pub(super) fn end_held(&mut self, lineage: Key, due: &mut Due<E, T>) {
        self.stop_held(
            |held| match &held.write {
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
    /// for.
    fn stop_held(
        &mut self,
        stopped: impl Fn(&HeldOp<E, T>) -> Option<Gate<E>>,
        due: &mut Due<E, T>,
    ) {
        if self.held.is_empty() {
            return;
        }
        let stopped: Vec<(u64, Gate<E>)> = self
            .held
            .iter()
            .filter_map(|(&seq, held)| stopped(held).map(|abandoned| (seq, abandoned)))
            .collect();
        for (seq, abandoned) in stopped {
            let Some(held) = self.held.remove(&seq) else {
                continue;
            };
            if let Some(write) = &held.write {
                if let Some(state) = self.lineages.get_mut(&write.lineage) {
                    state.gated.remove(&(write.epoch, seq));
                }
                self.unhold(write.lineage, write.epoch);
            }
            if !held.ready
                && let Some(state) = self.lineages.get_mut(&held.gate.lineage)
            {
                state.waiting.remove(&(held.gate.epoch, seq));
            }
            due.events.push(Event::NeverIssued {
                op: held.op,
                abandoned,
            });
        }
    }

    /// One held write of `lineage` at `epoch` is no longer held.
    fn unhold(&mut self, lineage: Key, epoch: E) {
        if let Some(state) = self.lineages.get_mut(&lineage)
            && let Some(count) = state.held_by_epoch.get_mut(&epoch)
        {
            *count -= 1;
            if *count == 0 {
                state.held_by_epoch.remove(&epoch);
            }
        }
    }
}
