// Copyright (c) Mike Grier.
use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
mod engine;
use crate::{Config as Limits, CreditEvent, Distribution, StopReason, failed, invalid};
pub use engine::run;

const MAX_KEYS: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub limits: Limits,
    pub keys: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arrangement {
    SharedState,
    KeyOwned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    Lookup,
    Add { value: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u64,
    pub key: usize,
    pub at_ns: u64,
    pub work: u32,
    pub operation: Operation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Completed { value: u64 },
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    pub key: usize,
    pub sequence: usize,
    pub worker: usize,
    pub scheduled_ns: u64,
    pub offered_ns: u64,
    pub admitted_ns: u64,
    pub started_ns: u64,
    pub committed_ns: u64,
    pub finished_ns: u64,
    pub collected_ns: u64,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerReport {
    pub worker: usize,
    pub completed: usize,
    pub cancelled: usize,
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
    pub request_slots: usize,
    pub reply_slots: usize,
    pub state_entries: usize,
    pub peak_outstanding: usize,
    pub key_peaks: Vec<usize>,
    pub lane_peaks: Vec<usize>,
    pub credit_full_observations: usize,
    pub lane_full_observations: Vec<usize>,
    pub trace: Vec<Request>,
    pub replies: Vec<Reply>,
    pub unadmitted: Vec<u64>,
    pub credit_events: Vec<CreditEvent>,
    pub workers: Vec<WorkerReport>,
    pub final_state: Vec<u64>,
}

pub fn owner(key: usize, workers: usize) -> usize {
    key % workers
}

pub fn validate(config: &Config, trace: &[Request]) -> io::Result<()> {
    if !(1..=MAX_KEYS).contains(&config.keys)
        || trace.len() > crate::MAX_REQUESTS
        || trace.iter().any(|request| request.key >= config.keys)
    {
        return Err(invalid("key count or request key outside stateful limits"));
    }
    let projected: Vec<_> = trace
        .iter()
        .map(|request| crate::Request {
            id: request.id,
            lane: 0,
            at_ns: request.at_ns,
            value: 0,
            work: request.work,
        })
        .collect();
    crate::validate(&config.limits, &projected)
}

pub fn trace(
    count: usize,
    keys: usize,
    burst: usize,
    interval_ns: u64,
    hot: bool,
    write_percent: usize,
    work: u32,
) -> io::Result<Vec<Request>> {
    if !(1..=MAX_KEYS).contains(&keys) || write_percent > 100 || work > crate::MAX_WORK {
        return Err(invalid("invalid key/mix/service trace limits"));
    }
    Ok(crate::trace(count, 1, burst, interval_ns, false)?
        .into_iter()
        .enumerate()
        .map(|(index, request)| Request {
            id: request.id,
            key: if hot && index % 8 != 0 {
                0
            } else if hot {
                (index / 8) % keys
            } else {
                index % keys
            },
            at_ns: request.at_ns,
            work: if index % 7 == 0 { work } else { work / 4 },
            operation: if (index * 37) % 100 < write_percent {
                Operation::Add {
                    value: if index % 11 == 0 {
                        u64::MAX
                    } else {
                        index as u64 + 1
                    },
                }
            } else {
                Operation::Lookup
            },
        })
        .collect())
}

pub fn verify(report: &Report) -> io::Result<()> {
    validate(&report.config, &report.trace)?;
    let limits = &report.config.limits;
    let keys = report.config.keys;
    if report.workers.len() != limits.workers
        || report.request_slots != limits.capacity
        || report.reply_slots != limits.capacity
        || report.state_entries != keys
        || report.final_state.len() != keys
        || report.key_peaks.len() != keys
        || report.lane_peaks.len() != limits.workers
        || report.lane_full_observations.len() != limits.workers
    {
        return Err(failed("stateful resource census mismatch"));
    }
    let mut replies = BTreeMap::new();
    let mut completed = vec![0; limits.workers];
    let mut cancelled = vec![0; limits.workers];
    let mut active = vec![0_u64; limits.workers];
    for reply in &report.replies {
        if replies.insert(reply.id, reply).is_some()
            || reply.worker >= limits.workers
            || reply.key >= keys
            || reply.scheduled_ns > reply.offered_ns
            || reply.offered_ns > reply.admitted_ns
            || reply.admitted_ns > reply.started_ns
            || reply.started_ns > reply.committed_ns
            || reply.committed_ns > reply.finished_ns
            || reply.finished_ns > reply.collected_ns
            || reply.collected_ns > report.wall_ns
            || (report.arrangement == Arrangement::KeyOwned
                && reply.worker != owner(reply.key, limits.workers))
        {
            return Err(failed("stateful reply identity/owner/timestamp violation"));
        }
        match reply.outcome {
            Outcome::Completed { .. } => completed[reply.worker] += 1,
            Outcome::Cancelled => cancelled[reply.worker] += 1,
        }
        active[reply.worker] = active[reply.worker]
            .checked_add(reply.finished_ns - reply.started_ns)
            .ok_or_else(|| failed("stateful active time overflow"))?;
    }
    let mut values = vec![0_u64; keys];
    let mut sequences = vec![0; keys];
    let mut last_commit = vec![0; keys];
    let mut stopped = vec![false; keys];
    let mut unadmitted = Vec::new();
    let mut gap = false;
    let mut matched = 0;
    for request in &report.trace {
        let Some(reply) = replies.get(&request.id) else {
            gap = true;
            unadmitted.push(request.id);
            continue;
        };
        if gap
            || reply.key != request.key
            || reply.sequence != sequences[request.key]
            || reply.scheduled_ns != request.at_ns
            || reply.committed_ns < last_commit[request.key]
        {
            return Err(failed("per-key commit order or admitted prefix violated"));
        }
        last_commit[request.key] = reply.committed_ns;
        sequences[request.key] += 1;
        matched += 1;
        match reply.outcome {
            Outcome::Completed { value } => {
                if stopped[request.key] {
                    return Err(failed("commit after cancelled key suffix"));
                }
                if let Operation::Add { value: increment } = request.operation {
                    values[request.key] = values[request.key].wrapping_add(increment);
                }
                if value != values[request.key] {
                    return Err(failed(
                        "lookup or state update disagrees with serial reference",
                    ));
                }
            }
            Outcome::Cancelled => {
                if report.stop == StopReason::Drained {
                    return Err(failed("cancelled effect in drained run"));
                }
                stopped[request.key] = true;
            }
        }
    }
    if matched != replies.len()
        || unadmitted != report.unadmitted
        || values != report.final_state
        || (report.stop == StopReason::Drained && !unadmitted.is_empty())
    {
        return Err(failed("terminal/unadmitted census or final state mismatch"));
    }
    let mut pending = BTreeSet::new();
    let mut next = 0;
    let mut previous = 0;
    let mut key_depth = vec![0_usize; keys];
    let mut key_peaks = key_depth.clone();
    let mut lane_depth = vec![0_usize; limits.workers];
    let mut lane_peaks = lane_depth.clone();
    let mut peak = 0;
    for event in &report.credit_events {
        match *event {
            CreditEvent::Admitted(id) => {
                let reply = replies
                    .get(&id)
                    .ok_or_else(|| failed("admission lacks terminal outcome"))?;
                if report
                    .trace
                    .get(next)
                    .is_none_or(|request| request.id != id)
                    || !pending.insert(id)
                    || reply.admitted_ns < previous
                {
                    return Err(failed("invalid stateful admission order"));
                }
                next += 1;
                previous = reply.admitted_ns;
                key_depth[reply.key] += 1;
                key_peaks[reply.key] = key_peaks[reply.key].max(key_depth[reply.key]);
                let lane = owner(reply.key, limits.workers);
                lane_depth[lane] += 1;
                lane_peaks[lane] = lane_peaks[lane].max(lane_depth[lane]);
                peak = peak.max(pending.len());
                if pending.len() > limits.capacity {
                    return Err(failed("stateful credit ceiling exceeded"));
                }
            }
            CreditEvent::Collected(id) => {
                if !pending.remove(&id) {
                    return Err(failed("collection without stateful credit"));
                }
                let reply = replies[&id];
                if reply.collected_ns < previous {
                    return Err(failed("stateful collection precedes admission"));
                }
                previous = reply.collected_ns;
                key_depth[reply.key] -= 1;
                lane_depth[owner(reply.key, limits.workers)] -= 1;
            }
        }
    }
    if !pending.is_empty()
        || next != replies.len()
        || peak != report.peak_outstanding
        || key_peaks != report.key_peaks
        || lane_peaks != report.lane_peaks
    {
        return Err(failed("stateful credit conservation mismatch"));
    }
    for (worker, stats) in report.workers.iter().enumerate() {
        if stats.worker != worker
            || stats.completed != completed[worker]
            || stats.cancelled != cancelled[worker]
            || stats.active_ns != active[worker]
        {
            return Err(failed("stateful worker census mismatch"));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub completed: Distribution,
    pub cancelled: Distribution,
    pub offer_lateness: Distribution,
    pub admission_wait: Distribution,
    pub queue_wait: Distribution,
    pub service_and_order_wait: Distribution,
}

pub fn summary(report: &Report, key: Option<usize>, lane: Option<usize>) -> io::Result<Summary> {
    verify(report)?;
    if key.is_some_and(|key| key >= report.config.keys)
        || lane.is_some_and(|lane| lane >= report.config.limits.workers)
    {
        return Err(invalid("summary key/lane outside configuration"));
    }
    let selected: Vec<_> = report
        .replies
        .iter()
        .filter(|reply| {
            key.is_none_or(|key| key == reply.key)
                && lane.is_none_or(|lane| owner(reply.key, report.config.limits.workers) == lane)
        })
        .collect();
    Ok(Summary {
        completed: Distribution::new(
            selected
                .iter()
                .filter(|reply| matches!(reply.outcome, Outcome::Completed { .. }))
                .map(|reply| reply.finished_ns - reply.scheduled_ns)
                .collect(),
        ),
        cancelled: Distribution::new(
            selected
                .iter()
                .filter(|reply| reply.outcome == Outcome::Cancelled)
                .map(|reply| reply.finished_ns - reply.scheduled_ns)
                .collect(),
        ),
        offer_lateness: Distribution::new(
            selected
                .iter()
                .map(|reply| reply.offered_ns - reply.scheduled_ns)
                .collect(),
        ),
        admission_wait: Distribution::new(
            selected
                .iter()
                .map(|reply| reply.admitted_ns - reply.offered_ns)
                .collect(),
        ),
        queue_wait: Distribution::new(
            selected
                .iter()
                .map(|reply| reply.started_ns - reply.admitted_ns)
                .collect(),
        ),
        service_and_order_wait: Distribution::new(
            selected
                .iter()
                .map(|reply| reply.committed_ns - reply.started_ns)
                .collect(),
        ),
    })
}

#[cfg(test)]
mod tests;
