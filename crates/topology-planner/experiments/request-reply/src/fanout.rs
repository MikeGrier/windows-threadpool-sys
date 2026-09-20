// Copyright (c) Mike Grier.
use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
mod engine;
pub use crate::Config;
use crate::{CreditEvent, Distribution, StopReason, failed, invalid};
pub use engine::run;

const MAX_UNITS: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arrangement {
    SerialWhole,
    OwnedJoin,
    ScatteredJoin,
}

/// Disjoint partition versus full copy. The two differ in flow and cardinality, not in
/// cost, and are deliberately not collapsed into one fan-out archetype.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Shape {
    Scatter { parts: u32 },
    Broadcast { branches: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Unit,
    Joiner,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub id: u64,
    pub at_ns: u64,
    pub value: u64,
    pub shape: Shape,
    pub work: u32,
    pub slow_unit: Option<u32>,
    pub fail_at: Option<u32>,
}

impl Record {
    pub fn units(&self) -> u32 {
        match self.shape {
            Shape::Scatter { parts } => parts,
            Shape::Broadcast { branches } => branches,
        }
    }

    pub fn unit_work(&self, index: u32) -> u32 {
        if self.slow_unit == Some(index) {
            self.work.saturating_mul(8).min(crate::MAX_WORK)
        } else {
            self.work
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Joined { value: u64 },
    Aborted { unit: u32 },
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitReport {
    pub index: u32,
    pub worker: usize,
    pub started_ns: u64,
    pub finished_ns: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Completion {
    pub id: u64,
    pub position: usize,
    pub joiner: usize,
    pub scheduled_ns: u64,
    pub offered_ns: u64,
    pub admitted_ns: u64,
    pub started_ns: u64,
    pub gathered_ns: u64,
    pub joined_ns: u64,
    pub collected_ns: u64,
    pub units: Vec<UnitReport>,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinedRecord {
    pub id: u64,
    pub value: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerReport {
    pub worker: usize,
    pub role: Role,
    pub units: usize,
    pub peak_units_held: usize,
    pub active_ns: u64,
    pub cpu_ns: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub evidence_class: String,
    pub placement: String,
    pub config: Config,
    pub arrangement: Arrangement,
    pub stop: StopReason,
    pub wall_ns: u64,
    pub record_slots: usize,
    pub unit_slots: usize,
    pub reply_slots: usize,
    pub peak_outstanding: usize,
    pub peak_partial_records: usize,
    pub peak_partial_units: usize,
    pub residual_partial_units: usize,
    pub discarded_partial_units: usize,
    pub credit_full_observations: usize,
    pub trace: Vec<Record>,
    pub completions: Vec<Completion>,
    pub unadmitted: Vec<u64>,
    pub credit_events: Vec<CreditEvent>,
    pub workers: Vec<WorkerReport>,
    pub log: Vec<JoinedRecord>,
}

/// A unit's result depends on the shape, so a unit of one shape cannot satisfy the other
/// and a collapsed archetype is detectable rather than merely discouraged.
pub fn unit_result(record: &Record, index: u32) -> u64 {
    match record.shape {
        Shape::Scatter { .. } => {
            record.value.rotate_left(index % 64) ^ u64::from(index).wrapping_add(0x5CA7)
        }
        Shape::Broadcast { .. } => record
            .value
            .wrapping_mul(u64::from(index).wrapping_add(1))
            .wrapping_add(0xB70AD),
    }
}

/// Commutative, because gather imposes no order among units. That is exactly why the
/// joined value cannot establish membership, which is checked structurally instead.
pub fn reference(record: &Record) -> u64 {
    (0..record.units()).fold(0_u64, |combined, index| {
        combined.wrapping_add(unit_result(record, index))
    })
}

pub fn owner(id: u64, workers: usize) -> usize {
    (id % workers as u64) as usize
}

fn unit_threads(config: &Config, arrangement: Arrangement) -> usize {
    match arrangement {
        Arrangement::SerialWhole => 1,
        Arrangement::OwnedJoin | Arrangement::ScatteredJoin => config.workers,
    }
}

fn threads(config: &Config, arrangement: Arrangement) -> usize {
    match arrangement {
        Arrangement::SerialWhole => 1,
        Arrangement::OwnedJoin => config.workers,
        Arrangement::ScatteredJoin => config.workers + 1,
    }
}

fn role_of(arrangement: Arrangement, index: usize, units: usize) -> Role {
    match arrangement {
        Arrangement::SerialWhole | Arrangement::OwnedJoin => Role::Owner,
        Arrangement::ScatteredJoin if index < units => Role::Unit,
        Arrangement::ScatteredJoin => Role::Joiner,
    }
}

pub fn validate(config: &Config, trace: &[Record]) -> io::Result<()> {
    for record in trace {
        let units = record.units();
        if !(1..=MAX_UNITS).contains(&units)
            || record.slow_unit.is_some_and(|unit| unit >= units)
            || record.fail_at.is_some_and(|unit| unit >= units)
            || record.unit_work(record.slow_unit.unwrap_or(0)) > crate::MAX_WORK
        {
            return Err(invalid(
                "unit count, slow unit or injected failure outside the record's own fan-out",
            ));
        }
    }
    let projected: Vec<_> = trace
        .iter()
        .map(|record| crate::Request {
            id: record.id,
            lane: 0,
            at_ns: record.at_ns,
            value: record.value,
            work: record.work,
        })
        .collect();
    crate::validate(config, &projected)
}

#[derive(Clone, Copy, Debug)]
pub struct TraceSpec {
    pub count: usize,
    pub burst: usize,
    pub interval_ns: u64,
    pub work: u32,
    pub units: u32,
    pub broadcast_every: usize,
    pub skew_every: usize,
    pub slow_every: usize,
    pub fail_every: usize,
}

impl Default for TraceSpec {
    fn default() -> Self {
        Self {
            count: 0,
            burst: 1,
            interval_ns: 0,
            work: 0,
            units: 1,
            broadcast_every: 0,
            skew_every: 0,
            slow_every: 0,
            fail_every: 0,
        }
    }
}

pub fn trace(spec: TraceSpec) -> io::Result<Vec<Record>> {
    if !(1..=MAX_UNITS).contains(&spec.units) || spec.work > crate::MAX_WORK {
        return Err(invalid("invalid fan-out trace dimensions"));
    }
    Ok(
        crate::trace(spec.count, 1, spec.burst, spec.interval_ns, false)?
            .into_iter()
            .enumerate()
            .map(|(index, request)| {
                let count =
                    if spec.skew_every != 0 && index % spec.skew_every == spec.skew_every - 1 {
                        spec.units
                    } else {
                        spec.units.div_ceil(2)
                    };
                Record {
                    id: request.id,
                    at_ns: request.at_ns,
                    value: request.value,
                    shape: if spec.broadcast_every != 0
                        && index % spec.broadcast_every == spec.broadcast_every - 1
                    {
                        Shape::Broadcast { branches: count }
                    } else {
                        Shape::Scatter { parts: count }
                    },
                    work: spec.work,
                    slow_unit: (spec.slow_every != 0
                        && index % spec.slow_every == spec.slow_every - 1)
                        .then(|| index as u32 % count),
                    fail_at: (spec.fail_every != 0
                        && index % spec.fail_every == spec.fail_every - 1)
                        .then(|| index as u32 % count),
                }
            })
            .collect(),
    )
}

/// Independent serial replay. Recomputes every unit result and joined value, checks each
/// join consumed its declared unit set exactly once, checks owner routing where the
/// arrangement declares it, and checks abort and cancellation left nothing behind.
pub fn verify(report: &Report) -> io::Result<()> {
    validate(&report.config, &report.trace)?;
    let config = &report.config;
    let units = unit_threads(config, report.arrangement);
    let joins = report.arrangement == Arrangement::ScatteredJoin;
    let (record_slots, unit_slots) = if joins {
        (0, config.capacity * MAX_UNITS as usize)
    } else {
        (config.capacity, 0)
    };
    if report.workers.len() != threads(config, report.arrangement)
        || report.record_slots != record_slots
        || report.unit_slots != unit_slots
        || report.reply_slots != config.capacity
        || report.residual_partial_units != 0
        || (!joins && (report.peak_partial_records != 0 || report.peak_partial_units != 0))
        || report.peak_partial_records > config.capacity
    {
        return Err(failed("fan-out resource census mismatch"));
    }
    let mut completions = BTreeMap::new();
    let mut per_worker_units = vec![0_usize; units];
    let mut per_worker_active = vec![0_u64; units];
    let (mut joined, mut aborted, mut cancelled, mut discarded) = (0, 0, 0, 0);
    for completion in &report.completions {
        if completions.insert(completion.id, completion).is_some()
            || completion.joiner >= report.workers.len()
            || completion.scheduled_ns > completion.offered_ns
            || completion.offered_ns > completion.admitted_ns
            || completion.admitted_ns > completion.started_ns
            || completion.started_ns > completion.gathered_ns
            || completion.gathered_ns > completion.joined_ns
            || completion.joined_ns > completion.collected_ns
            || completion.collected_ns > report.wall_ns
        {
            return Err(failed("fan-out identity or stage-timestamp violation"));
        }
        match completion.outcome {
            Outcome::Joined { .. } => joined += 1,
            Outcome::Aborted { .. } => {
                aborted += 1;
                discarded += completion.units.len();
            }
            Outcome::Cancelled => {
                cancelled += 1;
                discarded += completion.units.len();
            }
        }
    }
    let mut unadmitted = Vec::new();
    let mut entries = 0;
    let mut gap = false;
    let mut stopping = false;
    for (position, record) in report.trace.iter().enumerate() {
        let Some(completion) = completions.get(&record.id) else {
            gap = true;
            unadmitted.push(record.id);
            continue;
        };
        if gap || completion.position != position || completion.scheduled_ns != record.at_ns {
            return Err(failed("admitted prefix or declared position violated"));
        }
        let expected = record.units();
        let mut seen = BTreeSet::new();
        let mut first = u64::MAX;
        let mut last = 0;
        let mut combined = 0_u64;
        for unit in &completion.units {
            if unit.index >= expected
                || !seen.insert(unit.index)
                || unit.worker >= units
                || unit.started_ns > unit.finished_ns
                || unit.started_ns < completion.started_ns
                || unit.finished_ns > completion.gathered_ns
                || (report.arrangement == Arrangement::OwnedJoin
                    && unit.worker != owner(record.id, config.workers))
                || (report.arrangement == Arrangement::SerialWhole && unit.worker != 0)
            {
                return Err(failed(
                    "unit membership, ownership or timestamp outside its record",
                ));
            }
            per_worker_units[unit.worker] += 1;
            per_worker_active[unit.worker] = per_worker_active[unit.worker]
                .checked_add(unit.finished_ns - unit.started_ns)
                .ok_or_else(|| failed("fan-out unit time overflow"))?;
            first = first.min(unit.started_ns);
            last = last.max(unit.finished_ns);
            combined = combined.wrapping_add(unit_result(record, unit.index));
        }
        if seen.len() != expected as usize
            || first != completion.started_ns
            || last != completion.gathered_ns
        {
            return Err(failed(
                "join did not consume its declared unit set exactly once",
            ));
        }
        if report.arrangement == Arrangement::OwnedJoin
            && completion.joiner != owner(record.id, config.workers)
        {
            return Err(failed("owned join assembled by the wrong worker"));
        }
        match completion.outcome {
            Outcome::Joined { value } => {
                if stopping
                    || record.fail_at.is_some()
                    || value != combined
                    || value != reference(record)
                {
                    return Err(failed(
                        "join after a stop, at an injected failure, or with the wrong value",
                    ));
                }
                let entry = report
                    .log
                    .get(entries)
                    .ok_or_else(|| failed("join log is missing an entry"))?;
                if entry.id != record.id || entry.value != value {
                    return Err(failed(
                        "join log order or value disagrees with the serial replay",
                    ));
                }
                entries += 1;
            }
            Outcome::Aborted { unit } => {
                if stopping || record.fail_at != Some(unit) {
                    return Err(failed(
                        "abort after a stop, or at a unit with no injected failure",
                    ));
                }
            }
            Outcome::Cancelled => {
                if report.stop == StopReason::Drained {
                    return Err(failed("cancelled outcome in a drained run"));
                }
                stopping = true;
            }
        }
    }
    if entries != report.log.len()
        || unadmitted != report.unadmitted
        || completions.len() != joined + aborted + cancelled
        || discarded != report.discarded_partial_units
        || (report.stop == StopReason::Drained && !unadmitted.is_empty())
    {
        return Err(failed(
            "join log, unadmitted census, outcome census or discard census mismatch",
        ));
    }
    let mut pending = BTreeSet::new();
    let mut next = 0;
    let mut previous = 0;
    let mut peak = 0;
    for event in &report.credit_events {
        match *event {
            CreditEvent::Admitted(id) => {
                let completion = completions
                    .get(&id)
                    .ok_or_else(|| failed("admission lacks a terminal outcome"))?;
                if report.trace.get(next).is_none_or(|record| record.id != id)
                    || !pending.insert(id)
                    || completion.admitted_ns < previous
                {
                    return Err(failed("invalid fan-out admission order"));
                }
                next += 1;
                previous = completion.admitted_ns;
                peak = peak.max(pending.len());
                if pending.len() > config.capacity {
                    return Err(failed("fan-out credit ceiling exceeded"));
                }
            }
            CreditEvent::Collected(id) => {
                if !pending.remove(&id) {
                    return Err(failed("collection without a fan-out credit"));
                }
                if completions[&id].collected_ns < previous {
                    return Err(failed("fan-out collection precedes admission"));
                }
                previous = completions[&id].collected_ns;
            }
        }
    }
    if !pending.is_empty() || next != completions.len() || peak != report.peak_outstanding {
        return Err(failed("fan-out credit conservation mismatch"));
    }
    for (index, stats) in report.workers.iter().enumerate() {
        let role = role_of(report.arrangement, index, units);
        let computes = role != Role::Joiner;
        if stats.worker != index
            || stats.role != role
            || stats.units != if computes { per_worker_units[index] } else { 0 }
            || stats.active_ns
                != if computes {
                    per_worker_active[index]
                } else {
                    0
                }
            || (role == Role::Unit && stats.peak_units_held != 0)
        {
            return Err(failed("fan-out worker census mismatch"));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub joined: Distribution,
    pub aborted: Distribution,
    pub cancelled: Distribution,
    pub offer_lateness: Distribution,
    pub admission_wait: Distribution,
    pub gather: Distribution,
    pub join_wait: Distribution,
    pub units_per_record: Distribution,
}

pub fn summary(report: &Report, shape_is_broadcast: Option<bool>) -> io::Result<Summary> {
    verify(report)?;
    let shapes: BTreeMap<u64, bool> = report
        .trace
        .iter()
        .map(|record| (record.id, matches!(record.shape, Shape::Broadcast { .. })))
        .collect();
    let selected: Vec<_> = report
        .completions
        .iter()
        .filter(|completion| {
            shape_is_broadcast.is_none_or(|wanted| shapes.get(&completion.id) == Some(&wanted))
        })
        .collect();
    let spans = |pick: fn(&Completion) -> u64| {
        Distribution::new(selected.iter().map(|completion| pick(completion)).collect())
    };
    let terminal = |keep: fn(&Outcome) -> bool| {
        Distribution::new(
            selected
                .iter()
                .filter(|completion| keep(&completion.outcome))
                .map(|completion| completion.joined_ns - completion.scheduled_ns)
                .collect(),
        )
    };
    Ok(Summary {
        joined: terminal(|outcome| matches!(outcome, Outcome::Joined { .. })),
        aborted: terminal(|outcome| matches!(outcome, Outcome::Aborted { .. })),
        cancelled: terminal(|outcome| matches!(outcome, Outcome::Cancelled)),
        offer_lateness: spans(|completion| completion.offered_ns - completion.scheduled_ns),
        admission_wait: spans(|completion| completion.admitted_ns - completion.offered_ns),
        gather: spans(|completion| completion.gathered_ns - completion.started_ns),
        join_wait: spans(|completion| completion.joined_ns - completion.gathered_ns),
        units_per_record: Distribution::new(
            selected
                .iter()
                .map(|completion| completion.units.len() as u64)
                .collect(),
        ),
    })
}

#[cfg(test)]
mod tests;
