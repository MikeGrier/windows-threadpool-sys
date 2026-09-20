// Copyright (c) Mike Grier.
#![cfg(windows)]

mod engine;
pub use engine::run;
pub mod ingest;
pub mod stateful;

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};

const MAX_WORKERS: usize = 16;
const MAX_CAPACITY: usize = 4096;
const MAX_REQUESTS: usize = 16384;
const MAX_WORK: u32 = 1_000_000;
const MAX_SPAN_NS: u64 = 1_000_000_000;
const MAX_TIMEOUT_MS: u64 = 5000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub workers: usize,
    pub capacity: usize,
    pub timeout_ms: u64,
    pub cancel_after_admitted: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u64,
    pub lane: usize,
    pub at_ns: u64,
    pub value: u64,
    pub work: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Arrangement {
    Shared,
    Assigned,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Drained,
    Cancelled,
    Deadline,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Completed { value: u64 },
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    pub lane: usize,
    pub worker: usize,
    pub scheduled_ns: u64,
    pub offered_ns: u64,
    pub admitted_ns: u64,
    pub started_ns: u64,
    pub finished_ns: u64,
    pub collected_ns: u64,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum CreditEvent {
    Admitted(u64),
    Collected(u64),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerReport {
    pub worker: usize,
    pub handled: usize,
    pub completed: usize,
    pub cancelled: usize,
    pub active_ns: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub evidence_class: String,
    pub config: Config,
    pub arrangement: Arrangement,
    pub stop: StopReason,
    pub wall_ns: u64,
    pub request_slots: usize,
    pub reply_slots: usize,
    pub peak_outstanding: usize,
    pub lane_peaks: Vec<usize>,
    pub credit_full_observations: usize,
    pub lane_full_observations: Vec<usize>,
    pub trace: Vec<Request>,
    pub replies: Vec<Reply>,
    pub unadmitted: Vec<u64>,
    pub credit_events: Vec<CreditEvent>,
    pub workers: Vec<WorkerReport>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn failed(message: &str) -> io::Error {
    io::Error::other(message)
}

pub fn validate(config: &Config, trace: &[Request]) -> io::Result<()> {
    if !(1..=MAX_WORKERS).contains(&config.workers)
        || config.capacity < config.workers
        || config.capacity > MAX_CAPACITY
        || !config.capacity.is_multiple_of(config.workers)
        || !(1..=MAX_TIMEOUT_MS).contains(&config.timeout_ms)
        || trace.len() > MAX_REQUESTS
    {
        return Err(invalid(
            "workers, credits, timeout or request count outside experiment bounds",
        ));
    }
    if config
        .cancel_after_admitted
        .is_some_and(|count| count > trace.len())
    {
        return Err(invalid("cancellation count exceeds trace length"));
    }
    let mut ids = BTreeSet::new();
    let mut previous = 0;
    for request in trace {
        if request.lane >= config.workers
            || request.at_ns < previous
            || request.at_ns > MAX_SPAN_NS
            || request.work > MAX_WORK
            || !ids.insert(request.id)
        {
            return Err(invalid(
                "invalid lane, arrival order/span, work or duplicate identity",
            ));
        }
        previous = request.at_ns;
    }
    Ok(())
}

pub fn reference(request: &Request) -> u64 {
    let work = u64::from(request.work);
    request.value.wrapping_add(work * (work + 1) / 2)
}

pub fn trace(
    count: usize,
    workers: usize,
    burst: usize,
    interval_ns: u64,
    skewed: bool,
) -> io::Result<Vec<Request>> {
    if count > MAX_REQUESTS || !(1..=MAX_WORKERS).contains(&workers) || burst == 0 {
        return Err(invalid("invalid trace dimensions"));
    }
    (0..count)
        .map(|index| {
            let at_ns = (index / burst) as u64;
            let at_ns = at_ns
                .checked_mul(interval_ns)
                .filter(|offset| *offset <= MAX_SPAN_NS)
                .ok_or_else(|| invalid("arrival span exceeds experiment bound"))?;
            Ok(Request {
                id: index as u64,
                lane: if skewed {
                    if index % 8 != 0 {
                        0
                    } else {
                        (index / 8) % workers
                    }
                } else {
                    index % workers
                },
                at_ns,
                value: u64::MAX.wrapping_sub(index as u64),
                work: if skewed && index % 8 == 1 {
                    100_000
                } else {
                    1000
                },
            })
        })
        .collect()
}

pub fn verify(report: &Report) -> io::Result<()> {
    validate(&report.config, &report.trace)?;
    let config = &report.config;
    if report.request_slots != config.capacity
        || report.reply_slots != config.capacity
        || report.workers.len() != config.workers
        || report.lane_peaks.len() != config.workers
        || report.lane_full_observations.len() != config.workers
    {
        return Err(failed("resource or worker census mismatch"));
    }
    let requests: BTreeMap<_, _> = report
        .trace
        .iter()
        .map(|request| (request.id, request))
        .collect();
    let mut remaining: BTreeSet<_> = requests.keys().copied().collect();
    let mut completed = vec![0; config.workers];
    let mut cancelled = vec![0; config.workers];
    let mut active = vec![0_u64; config.workers];
    let mut replies = BTreeMap::new();
    for reply in &report.replies {
        let Some(request) = requests.get(&reply.id) else {
            return Err(failed("unknown reply identity"));
        };
        if !remaining.remove(&reply.id)
            || reply.worker >= config.workers
            || reply.lane != request.lane
            || (report.arrangement == Arrangement::Assigned && reply.worker != request.lane)
            || reply.scheduled_ns != request.at_ns
            || reply.scheduled_ns > reply.offered_ns
            || reply.offered_ns > reply.admitted_ns
            || reply.admitted_ns > reply.started_ns
            || reply.started_ns > reply.finished_ns
            || reply.finished_ns > reply.collected_ns
            || reply.collected_ns > report.wall_ns
        {
            return Err(failed(
                "reply identity, owner or timestamp contract violated",
            ));
        }
        match reply.outcome {
            Outcome::Completed { value } if value == reference(request) => {
                completed[reply.worker] += 1
            }
            Outcome::Cancelled if report.stop != StopReason::Drained => {
                cancelled[reply.worker] += 1
            }
            _ => return Err(failed("incorrect result or unexpected cancellation")),
        }
        active[reply.worker] = active[reply.worker]
            .checked_add(reply.finished_ns - reply.started_ns)
            .ok_or_else(|| failed("active time overflow"))?;
        replies.insert(reply.id, reply);
    }
    let unadmitted: BTreeSet<_> = report.unadmitted.iter().copied().collect();
    if remaining != unadmitted
        || unadmitted.len() != report.unadmitted.len()
        || (report.stop == StopReason::Drained && !remaining.is_empty())
    {
        return Err(failed("unadmitted request census mismatch"));
    }
    let mut pending = BTreeSet::new();
    let mut admitted = BTreeSet::new();
    let mut lane_depths = vec![0_usize; config.workers];
    let mut lane_peaks = vec![0_usize; config.workers];
    let mut peak = 0;
    let mut next = 0;
    let mut previous_event_ns = 0;
    for event in &report.credit_events {
        match *event {
            CreditEvent::Admitted(id) => {
                let Some(reply) = replies.get(&id) else {
                    return Err(failed("admission missing terminal reply"));
                };
                if report
                    .trace
                    .get(next)
                    .is_none_or(|request| request.id != id)
                    || reply.admitted_ns < previous_event_ns
                {
                    return Err(failed("admission is not the scheduled trace prefix"));
                }
                next += 1;
                previous_event_ns = reply.admitted_ns;
                if !admitted.insert(id) || !pending.insert(id) {
                    return Err(failed("duplicate admission"));
                }
                lane_depths[reply.lane] += 1;
                lane_peaks[reply.lane] = lane_peaks[reply.lane].max(lane_depths[reply.lane]);
                peak = peak.max(pending.len());
                if pending.len() > config.capacity {
                    return Err(failed("global credit ceiling exceeded"));
                }
            }
            CreditEvent::Collected(id) => {
                if !pending.remove(&id) {
                    return Err(failed("collection without outstanding credit"));
                }
                if replies[&id].collected_ns < previous_event_ns {
                    return Err(failed("collection timestamp precedes credit events"));
                }
                previous_event_ns = replies[&id].collected_ns;
                lane_depths[replies[&id].lane] -= 1;
            }
        }
    }
    if !pending.is_empty()
        || admitted.len() != report.replies.len()
        || peak != report.peak_outstanding
        || lane_peaks != report.lane_peaks
    {
        return Err(failed("credit conservation or peak mismatch"));
    }
    if report.unadmitted
        != report.trace[next..]
            .iter()
            .map(|request| request.id)
            .collect::<Vec<_>>()
    {
        return Err(failed(
            "unadmitted identities are not the remaining trace suffix",
        ));
    }
    for (worker, stats) in report.workers.iter().enumerate() {
        if stats.worker != worker
            || stats.completed != completed[worker]
            || stats.cancelled != cancelled[worker]
            || stats.handled != completed[worker] + cancelled[worker]
            || stats.active_ns != active[worker]
        {
            return Err(failed("worker outcome census mismatch"));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Distribution {
    pub count: usize,
    pub p50_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
}

impl Distribution {
    pub(crate) fn new(mut values: Vec<u64>) -> Self {
        values.sort_unstable();
        let count = values.len();
        let percentile = |percent: usize| {
            if count == 0 {
                0
            } else {
                values[(count * percent).div_ceil(100) - 1]
            }
        };
        Self {
            count,
            p50_ns: percentile(50),
            p99_ns: percentile(99),
            max_ns: values.last().copied().unwrap_or(0),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ResponseSummary {
    pub completed: Distribution,
    pub cancelled: Distribution,
    pub offer_lateness: Distribution,
    pub admission_wait: Distribution,
    pub queue_wait: Distribution,
    pub collection_delay: Distribution,
}

pub fn response_summary(report: &Report, lane: Option<usize>) -> io::Result<ResponseSummary> {
    verify(report)?;
    if lane.is_some_and(|lane| lane >= report.config.workers) {
        return Err(invalid("summary lane outside configuration"));
    }
    let selected: Vec<_> = report
        .replies
        .iter()
        .filter(|reply| lane.is_none_or(|lane| reply.lane == lane))
        .collect();
    Ok(ResponseSummary {
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
        collection_delay: Distribution::new(
            selected
                .iter()
                .map(|reply| reply.collected_ns - reply.finished_ns)
                .collect(),
        ),
    })
}

#[cfg(test)]
mod tests;
