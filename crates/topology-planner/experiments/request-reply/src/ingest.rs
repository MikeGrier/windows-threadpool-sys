// Copyright (c) Mike Grier.
use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
mod engine;
use crate::{Config as Limits, CreditEvent, Distribution, StopReason, failed, invalid};
pub use engine::run;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub limits: Limits,
    pub reorder_capacity: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arrangement {
    SerialOwner,
    StagedPipeline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Transform,
    Publish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Transform,
    Publisher,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub id: u64,
    pub at_ns: u64,
    pub value: u64,
    pub transform_work: u32,
    pub publish_work: u32,
    pub fail_at: Option<Stage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Published { value: u64 },
    Aborted { stage: Stage },
    Cancelled,
}

/// One admitted record's stage timestamps. `published_ns` is when the publication
/// cursor reached it, so `published_ns - ready_ns` is head-of-line delay alone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Completion {
    pub id: u64,
    pub position: usize,
    pub worker: usize,
    pub scheduled_ns: u64,
    pub offered_ns: u64,
    pub admitted_ns: u64,
    pub started_ns: u64,
    pub ready_ns: u64,
    pub published_ns: u64,
    pub finished_ns: u64,
    pub collected_ns: u64,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    pub id: u64,
    pub value: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerReport {
    pub worker: usize,
    pub role: Role,
    pub transformed: usize,
    pub published: usize,
    pub aborted: usize,
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
    pub reorder_slots: usize,
    pub peak_outstanding: usize,
    pub peak_reorder_occupancy: usize,
    pub credit_full_observations: usize,
    pub reorder_full_observations: usize,
    pub trace: Vec<Record>,
    pub completions: Vec<Completion>,
    pub unadmitted: Vec<u64>,
    pub credit_events: Vec<CreditEvent>,
    pub workers: Vec<WorkerReport>,
    pub log: Vec<Published>,
}

pub fn transformers(config: &Config, arrangement: Arrangement) -> usize {
    match arrangement {
        Arrangement::SerialOwner => 1,
        Arrangement::StagedPipeline => config.limits.workers,
    }
}

pub fn reference(record: &Record) -> u64 {
    let work = u64::from(record.transform_work);
    record.value.wrapping_add(work * (work + 1) / 2)
}

fn threads(config: &Config, arrangement: Arrangement) -> usize {
    match arrangement {
        Arrangement::SerialOwner => 1,
        Arrangement::StagedPipeline => config.limits.workers + 1,
    }
}

fn reorder_slots(config: &Config, arrangement: Arrangement) -> usize {
    match arrangement {
        Arrangement::SerialOwner => 0,
        Arrangement::StagedPipeline => config.reorder_capacity,
    }
}

pub fn validate(config: &Config, trace: &[Record]) -> io::Result<()> {
    if !(1..=config.limits.capacity).contains(&config.reorder_capacity)
        || trace
            .iter()
            .any(|record| record.publish_work > crate::MAX_WORK)
    {
        return Err(invalid(
            "reorder capacity or publish work outside ingestion limits",
        ));
    }
    let projected: Vec<_> = trace
        .iter()
        .map(|record| crate::Request {
            id: record.id,
            lane: 0,
            at_ns: record.at_ns,
            value: record.value,
            work: record.transform_work,
        })
        .collect();
    crate::validate(&config.limits, &projected)
}

pub fn trace(
    count: usize,
    burst: usize,
    interval_ns: u64,
    transform_work: u32,
    publish_work: u32,
    slow_first: bool,
    fail_every: usize,
) -> io::Result<Vec<Record>> {
    if transform_work > crate::MAX_WORK || publish_work > crate::MAX_WORK {
        return Err(invalid("invalid ingestion service work"));
    }
    Ok(crate::trace(count, 1, burst, interval_ns, false)?
        .into_iter()
        .enumerate()
        .map(|(index, request)| Record {
            id: request.id,
            at_ns: request.at_ns,
            value: request.value,
            transform_work: if slow_first && index == 0 {
                transform_work.saturating_mul(8).min(crate::MAX_WORK)
            } else if index % 7 == 0 {
                transform_work
            } else {
                transform_work / 4
            },
            publish_work,
            fail_at: match fail_every {
                0 => None,
                every if index % every != every - 1 => None,
                every if (index / every) % 2 == 0 => Some(Stage::Transform),
                _ => Some(Stage::Publish),
            },
        })
        .collect())
}

/// Independent serial replay. Recomputes every published value, checks the log is
/// exactly the published subsequence in declared order, checks each record's own
/// transform-before-publish timestamps, and checks abort and cancellation left no
/// entry behind. Log equality alone would not establish any of those.
pub fn verify(report: &Report) -> io::Result<()> {
    validate(&report.config, &report.trace)?;
    let limits = &report.config.limits;
    let transformers = transformers(&report.config, report.arrangement);
    let slots = reorder_slots(&report.config, report.arrangement);
    if report.workers.len() != threads(&report.config, report.arrangement)
        || report.request_slots != limits.capacity
        || report.reply_slots != limits.capacity
        || report.reorder_slots != slots
        || report.peak_reorder_occupancy > slots
        || (report.arrangement == Arrangement::SerialOwner && report.reorder_full_observations != 0)
    {
        return Err(failed("ingestion resource census mismatch"));
    }
    let mut completions = BTreeMap::new();
    let mut transformed = vec![0_usize; transformers];
    let mut transform_active = vec![0_u64; transformers];
    let mut publish_active = 0_u64;
    let (mut published, mut aborted, mut cancelled) = (0, 0, 0);
    for completion in &report.completions {
        if completions.insert(completion.id, completion).is_some()
            || completion.worker >= transformers
            || completion.scheduled_ns > completion.offered_ns
            || completion.offered_ns > completion.admitted_ns
            || completion.admitted_ns > completion.started_ns
            || completion.started_ns > completion.ready_ns
            || completion.ready_ns > completion.published_ns
            || completion.published_ns > completion.finished_ns
            || completion.finished_ns > completion.collected_ns
            || completion.collected_ns > report.wall_ns
        {
            return Err(failed(
                "ingestion identity, worker or stage-timestamp violation",
            ));
        }
        transformed[completion.worker] += 1;
        transform_active[completion.worker] = transform_active[completion.worker]
            .checked_add(completion.ready_ns - completion.started_ns)
            .ok_or_else(|| failed("ingestion transform time overflow"))?;
        publish_active = publish_active
            .checked_add(completion.finished_ns - completion.published_ns)
            .ok_or_else(|| failed("ingestion publish time overflow"))?;
        match completion.outcome {
            Outcome::Published { .. } => published += 1,
            Outcome::Aborted { .. } => aborted += 1,
            Outcome::Cancelled => cancelled += 1,
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
            return Err(failed(
                "admitted prefix or declared publication position violated",
            ));
        }
        match completion.outcome {
            Outcome::Published { value } => {
                if stopping || record.fail_at.is_some() || value != reference(record) {
                    return Err(failed(
                        "publication after a stop, at an injected failure, or with the wrong value",
                    ));
                }
                let entry = report
                    .log
                    .get(entries)
                    .ok_or_else(|| failed("publication log is missing an entry"))?;
                if entry.id != record.id || entry.value != value {
                    return Err(failed(
                        "publication log order or value disagrees with the serial replay",
                    ));
                }
                entries += 1;
            }
            Outcome::Aborted { stage } => {
                if stopping || record.fail_at != Some(stage) {
                    return Err(failed(
                        "abort after a stop, or at a stage with no injected failure",
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
        || completions.len() != published + aborted + cancelled
        || (report.stop == StopReason::Drained && !unadmitted.is_empty())
    {
        return Err(failed(
            "publication log, unadmitted census or outcome census mismatch",
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
                    return Err(failed("invalid ingestion admission order"));
                }
                next += 1;
                previous = completion.admitted_ns;
                peak = peak.max(pending.len());
                if pending.len() > limits.capacity {
                    return Err(failed("ingestion credit ceiling exceeded"));
                }
            }
            CreditEvent::Collected(id) => {
                if !pending.remove(&id) {
                    return Err(failed("collection without an ingestion credit"));
                }
                if completions[&id].collected_ns < previous {
                    return Err(failed("ingestion collection precedes admission"));
                }
                previous = completions[&id].collected_ns;
            }
        }
    }
    if !pending.is_empty() || next != completions.len() || peak != report.peak_outstanding {
        return Err(failed("ingestion credit conservation mismatch"));
    }
    for (index, stats) in report.workers.iter().enumerate() {
        let role = match report.arrangement {
            Arrangement::SerialOwner => Role::Owner,
            Arrangement::StagedPipeline if index < transformers => Role::Transform,
            Arrangement::StagedPipeline => Role::Publisher,
        };
        let publishes = role != Role::Transform;
        let expected = WorkerReport {
            worker: index,
            role,
            transformed: if role == Role::Publisher {
                0
            } else {
                transformed[index]
            },
            published: if publishes { published } else { 0 },
            aborted: if publishes { aborted } else { 0 },
            cancelled: if publishes { cancelled } else { 0 },
            active_ns: match role {
                Role::Owner => transform_active[index] + publish_active,
                Role::Transform => transform_active[index],
                Role::Publisher => publish_active,
            },
            cpu_ns: stats.cpu_ns,
        };
        if stats.worker != expected.worker
            || stats.role != expected.role
            || stats.transformed != expected.transformed
            || stats.published != expected.published
            || stats.aborted != expected.aborted
            || stats.cancelled != expected.cancelled
            || stats.active_ns != expected.active_ns
        {
            return Err(failed("ingestion worker census mismatch"));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub published: Distribution,
    pub aborted: Distribution,
    pub cancelled: Distribution,
    pub offer_lateness: Distribution,
    pub admission_wait: Distribution,
    pub queue_wait: Distribution,
    pub transform: Distribution,
    pub head_of_line: Distribution,
    pub publish: Distribution,
}

pub fn summary(report: &Report, worker: Option<usize>) -> io::Result<Summary> {
    verify(report)?;
    if worker.is_some_and(|worker| worker >= transformers(&report.config, report.arrangement)) {
        return Err(invalid("summary worker outside configuration"));
    }
    let selected: Vec<_> = report
        .completions
        .iter()
        .filter(|completion| worker.is_none_or(|worker| worker == completion.worker))
        .collect();
    let spans = |pick: fn(&Completion) -> u64| {
        Distribution::new(selected.iter().map(|completion| pick(completion)).collect())
    };
    let terminal = |keep: fn(&Outcome) -> bool| {
        Distribution::new(
            selected
                .iter()
                .filter(|completion| keep(&completion.outcome))
                .map(|completion| completion.finished_ns - completion.scheduled_ns)
                .collect(),
        )
    };
    Ok(Summary {
        published: terminal(|outcome| matches!(outcome, Outcome::Published { .. })),
        aborted: terminal(|outcome| matches!(outcome, Outcome::Aborted { .. })),
        cancelled: terminal(|outcome| matches!(outcome, Outcome::Cancelled)),
        offer_lateness: spans(|completion| completion.offered_ns - completion.scheduled_ns),
        admission_wait: spans(|completion| completion.admitted_ns - completion.offered_ns),
        queue_wait: spans(|completion| completion.started_ns - completion.admitted_ns),
        transform: spans(|completion| completion.ready_ns - completion.started_ns),
        head_of_line: spans(|completion| completion.published_ns - completion.ready_ns),
        publish: spans(|completion| completion.finished_ns - completion.published_ns),
    })
}

#[cfg(test)]
mod tests;
