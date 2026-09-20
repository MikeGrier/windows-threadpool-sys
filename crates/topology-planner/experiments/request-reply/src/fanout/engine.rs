// Copyright (c) Mike Grier.
use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

const CHECK_INTERVAL: Duration = Duration::from_millis(1);
const SERVICE_CHUNK: u32 = 256;

struct RecordJob {
    record: Record,
    position: usize,
    offered_ns: u64,
    admitted_ns: u64,
}

struct UnitJob {
    record: Record,
    position: usize,
    index: u32,
    offered_ns: u64,
    admitted_ns: u64,
}

#[derive(Clone, Copy)]
enum UnitKind {
    Computed { value: u64 },
    Failed,
    Interrupted,
}

struct UnitOutcome {
    record: Record,
    position: usize,
    offered_ns: u64,
    admitted_ns: u64,
    report: UnitReport,
    kind: UnitKind,
}

enum GatherResult {
    Complete { value: u64 },
    Failed { unit: u32 },
    Interrupted,
}

struct Gathered {
    id: u64,
    position: usize,
    joiner: usize,
    offered_ns: u64,
    admitted_ns: u64,
    units: Vec<UnitReport>,
    result: GatherResult,
}

struct Session<'a> {
    start: Instant,
    deadline: Instant,
    cancel: &'a AtomicBool,
    abort: &'a AtomicBool,
}

type Service = dyn Fn(&Record, u32, &AtomicBool, Instant) -> io::Result<bool> + Sync;
struct Hooks<'a> {
    service: &'a Service,
    cpu: &'a (dyn Fn() -> io::Result<u64> + Sync),
}

fn elapsed(start: Instant) -> u64 {
    start.elapsed().as_nanos() as u64
}

fn cpu_ns() -> io::Result<u64> {
    let mut created: FILETIME = unsafe { std::mem::zeroed() };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    if unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Ok((ticks(kernel) + ticks(user)) * 100)
}

fn service(
    record: &Record,
    index: u32,
    cancel: &AtomicBool,
    deadline: Instant,
) -> io::Result<bool> {
    let work = record.unit_work(index);
    let mut done = 0;
    let mut value = record.id.wrapping_add(u64::from(index));
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Ok(false);
        }
        if done == work {
            return Ok(true);
        }
        let end = work.min(done + SERVICE_CHUNK);
        for step in done..end {
            value = std::hint::black_box(value.wrapping_add(u64::from(step)));
        }
        done = end;
    }
}

/// Every unit of a record is attempted, including after one has failed, so membership is
/// uniformly exact across arrangements and an abort discards a complete unit set.
fn compute(
    record: &Record,
    index: u32,
    worker: usize,
    session: &Session<'_>,
    hooks: &Hooks<'_>,
) -> io::Result<(UnitReport, UnitKind)> {
    let started_ns = elapsed(session.start);
    let kind = if record.fail_at == Some(index) {
        UnitKind::Failed
    } else if (hooks.service)(record, index, session.cancel, session.deadline)? {
        UnitKind::Computed {
            value: unit_result(record, index),
        }
    } else {
        UnitKind::Interrupted
    };
    Ok((
        UnitReport {
            index,
            worker,
            started_ns,
            finished_ns: elapsed(session.start),
        },
        kind,
    ))
}

fn fold(kinds: &[UnitKind]) -> GatherResult {
    let mut combined = 0_u64;
    let mut failed = None;
    let mut interrupted = false;
    for (index, kind) in kinds.iter().enumerate() {
        match kind {
            UnitKind::Computed { value } => combined = combined.wrapping_add(*value),
            UnitKind::Failed => failed = failed.or(Some(index as u32)),
            UnitKind::Interrupted => interrupted = true,
        }
    }
    match (failed, interrupted) {
        (Some(unit), _) => GatherResult::Failed { unit },
        (None, true) => GatherResult::Interrupted,
        (None, false) => GatherResult::Complete { value: combined },
    }
}

struct FailureGuard<'a> {
    session: &'a Session<'a>,
    armed: bool,
}
impl Drop for FailureGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.session.abort.store(true, Ordering::Release);
            self.session.cancel.store(true, Ordering::Release);
        }
    }
}

struct Produced {
    report: WorkerReport,
    peak_partial_records: usize,
    peak_partial_units: usize,
    residual_partial_units: usize,
}

/// `SerialWhole` and `OwnedJoin`: the worker computes every unit of its records and joins
/// them itself, so no partial state ever crosses a thread.
fn own(
    index: usize,
    requests: Receiver<RecordJob>,
    gathered: Sender<Gathered>,
    session: &Session<'_>,
    hooks: &Hooks<'_>,
) -> io::Result<Produced> {
    let initial = (hooks.cpu)()?;
    let mut report = WorkerReport {
        worker: index,
        role: Role::Owner,
        units: 0,
        peak_units_held: 0,
        active_ns: 0,
        cpu_ns: 0,
    };
    while let Ok(job) = requests.recv() {
        if session.abort.load(Ordering::Acquire) {
            return Err(failed("peer failed"));
        }
        let mut units = Vec::new();
        let mut kinds = Vec::new();
        for unit in 0..job.record.units() {
            let (report_unit, kind) = compute(&job.record, unit, index, session, hooks)?;
            report.active_ns += report_unit.finished_ns - report_unit.started_ns;
            units.push(report_unit);
            kinds.push(kind);
            report.peak_units_held = report.peak_units_held.max(units.len());
        }
        report.units += units.len();
        gathered
            .send(Gathered {
                id: job.record.id,
                position: job.position,
                joiner: index,
                offered_ns: job.offered_ns,
                admitted_ns: job.admitted_ns,
                units,
                result: fold(&kinds),
            })
            .map_err(|_| failed("fan-out collector disconnected"))?;
    }
    report.cpu_ns = (hooks.cpu)()?.saturating_sub(initial);
    Ok(Produced {
        report,
        peak_partial_records: 0,
        peak_partial_units: 0,
        residual_partial_units: 0,
    })
}

/// `ScatteredJoin` unit worker: any worker may take any unit of any record.
fn unit_worker(
    index: usize,
    units: Receiver<UnitJob>,
    outcomes: Sender<UnitOutcome>,
    session: &Session<'_>,
    hooks: &Hooks<'_>,
) -> io::Result<Produced> {
    let initial = (hooks.cpu)()?;
    let mut report = WorkerReport {
        worker: index,
        role: Role::Unit,
        units: 0,
        peak_units_held: 0,
        active_ns: 0,
        cpu_ns: 0,
    };
    while let Ok(job) = units.recv() {
        if session.abort.load(Ordering::Acquire) {
            return Err(failed("peer failed"));
        }
        let (unit, kind) = compute(&job.record, job.index, index, session, hooks)?;
        report.active_ns += unit.finished_ns - unit.started_ns;
        report.units += 1;
        outcomes
            .send(UnitOutcome {
                record: job.record,
                position: job.position,
                offered_ns: job.offered_ns,
                admitted_ns: job.admitted_ns,
                report: unit,
                kind,
            })
            .map_err(|_| failed("fan-out joiner disconnected"))?;
    }
    report.cpu_ns = (hooks.cpu)()?.saturating_sub(initial);
    Ok(Produced {
        report,
        peak_partial_records: 0,
        peak_partial_units: 0,
        residual_partial_units: 0,
    })
}

struct Partial {
    record: Record,
    position: usize,
    offered_ns: u64,
    admitted_ns: u64,
    entries: Vec<(UnitReport, UnitKind)>,
}

/// `ScatteredJoin` joiner: the only component that holds partial state for records it did
/// not compute, which is what the peak/residual counters measure.
fn joiner(
    index: usize,
    outcomes: Receiver<UnitOutcome>,
    gathered: Sender<Gathered>,
    hooks: &Hooks<'_>,
) -> io::Result<Produced> {
    let initial = (hooks.cpu)()?;
    let mut report = WorkerReport {
        worker: index,
        role: Role::Joiner,
        units: 0,
        peak_units_held: 0,
        active_ns: 0,
        cpu_ns: 0,
    };
    let mut partials: BTreeMap<u64, Partial> = BTreeMap::new();
    let mut peak_records = 0;
    let mut peak_units = 0;
    while let Ok(outcome) = outcomes.recv() {
        let expected = outcome.record.units() as usize;
        let partial = partials
            .entry(outcome.record.id)
            .or_insert_with(|| Partial {
                record: outcome.record.clone(),
                position: outcome.position,
                offered_ns: outcome.offered_ns,
                admitted_ns: outcome.admitted_ns,
                entries: Vec::with_capacity(expected),
            });
        partial.entries.push((outcome.report, outcome.kind));
        let held: usize = partials.values().map(|partial| partial.entries.len()).sum();
        peak_records = peak_records.max(partials.len());
        peak_units = peak_units.max(held);
        report.peak_units_held = peak_units;
        if partials[&outcome.record.id].entries.len() == expected {
            let mut partial = partials.remove(&outcome.record.id).unwrap();
            partial.entries.sort_by_key(|(unit, _)| unit.index);
            let kinds: Vec<_> = partial.entries.iter().map(|(_, kind)| *kind).collect();
            let units: Vec<_> = partial.entries.into_iter().map(|(unit, _)| unit).collect();
            gathered
                .send(Gathered {
                    id: partial.record.id,
                    position: partial.position,
                    joiner: index,
                    offered_ns: partial.offered_ns,
                    admitted_ns: partial.admitted_ns,
                    units,
                    result: fold(&kinds),
                })
                .map_err(|_| failed("fan-out collector disconnected"))?;
        }
    }
    report.cpu_ns = (hooks.cpu)()?.saturating_sub(initial);
    let residual = partials.values().map(|partial| partial.entries.len()).sum();
    Ok(Produced {
        report,
        peak_partial_records: peak_records,
        peak_partial_units: peak_units,
        residual_partial_units: residual,
    })
}

/// Declared order is resolved here rather than in a worker, so this path holds the
/// resequencing rule RR-D8 established constant while fan-out varies.
struct Publication {
    cancelling: bool,
    cursor: usize,
    ready: BTreeMap<usize, Gathered>,
}

struct Channels<'a> {
    records: &'a [Sender<RecordJob>],
    units: &'a Sender<UnitJob>,
    gathered: &'a Receiver<Gathered>,
}

fn coordinate(
    config: &Config,
    trace: &[Record],
    arrangement: Arrangement,
    channels: &Channels<'_>,
    external: &AtomicBool,
    session: &Session<'_>,
) -> io::Result<Report> {
    let Channels {
        records,
        units,
        gathered,
    } = *channels;
    let mut report = Report {
        schema: "fanout-v1".into(),
        evidence_class: "offline_unpinned_fan_out_join".into(),
        placement: "unpinned_host_threads_no_numa_claim".into(),
        config: config.clone(),
        arrangement,
        stop: StopReason::Drained,
        wall_ns: 0,
        record_slots: records
            .iter()
            .map(|sender| sender.capacity().unwrap())
            .sum(),
        unit_slots: match arrangement {
            Arrangement::ScatteredJoin => units.capacity().unwrap(),
            _ => 0,
        },
        reply_slots: gathered.capacity().unwrap(),
        peak_outstanding: 0,
        peak_partial_records: 0,
        peak_partial_units: 0,
        residual_partial_units: 0,
        discarded_partial_units: 0,
        credit_full_observations: 0,
        trace: trace.to_vec(),
        completions: Vec::with_capacity(trace.len()),
        unadmitted: vec![],
        credit_events: Vec::with_capacity(trace.len() * 2),
        workers: vec![],
        log: vec![],
    };
    let mut publication = Publication {
        cancelling: false,
        cursor: 0,
        ready: BTreeMap::new(),
    };
    let mut pending = BTreeMap::new();
    let mut next = 0;
    let mut offered = None;
    let resolve = |publication: &mut Publication,
                   report: &mut Report,
                   pending: &mut BTreeMap<u64, usize>|
     -> io::Result<()> {
        while let Some(item) = publication.ready.remove(&publication.cursor) {
            if pending.remove(&item.id) != Some(item.position) {
                return Err(failed("fan-out completion correlation mismatch"));
            }
            let joined_ns = elapsed(session.start);
            let outcome = if publication.cancelling {
                Outcome::Cancelled
            } else {
                match item.result {
                    GatherResult::Interrupted => {
                        publication.cancelling = true;
                        Outcome::Cancelled
                    }
                    GatherResult::Failed { unit } => Outcome::Aborted { unit },
                    GatherResult::Complete { value } => {
                        report.log.push(JoinedRecord { id: item.id, value });
                        Outcome::Joined { value }
                    }
                }
            };
            if !matches!(outcome, Outcome::Joined { .. }) {
                report.discarded_partial_units += item.units.len();
            }
            let started_ns = item
                .units
                .iter()
                .map(|unit| unit.started_ns)
                .min()
                .unwrap_or(joined_ns);
            let gathered_ns = item
                .units
                .iter()
                .map(|unit| unit.finished_ns)
                .max()
                .unwrap_or(joined_ns);
            report.credit_events.push(CreditEvent::Collected(item.id));
            report.completions.push(Completion {
                id: item.id,
                position: item.position,
                joiner: item.joiner,
                scheduled_ns: trace[item.position].at_ns,
                offered_ns: item.offered_ns,
                admitted_ns: item.admitted_ns,
                started_ns,
                gathered_ns,
                joined_ns,
                collected_ns: elapsed(session.start),
                units: item.units,
                outcome,
            });
            publication.cursor += 1;
        }
        Ok(())
    };
    while next < trace.len() || !pending.is_empty() {
        if session.abort.load(Ordering::Acquire) {
            return Err(failed("fan-out worker failed"));
        }
        loop {
            match gathered.try_recv() {
                Ok(item) => {
                    publication.ready.insert(item.position, item);
                    resolve(&mut publication, &mut report, &mut pending)?;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(failed("fan-out results disconnected"));
                }
            }
        }
        if report.stop == StopReason::Drained {
            if external.load(Ordering::Acquire)
                || config
                    .cancel_after_admitted
                    .is_some_and(|count| next >= count)
            {
                report.stop = StopReason::Cancelled;
            } else if Instant::now() >= session.deadline {
                report.stop = StopReason::Deadline;
            }
            if report.stop != StopReason::Drained {
                session.cancel.store(true, Ordering::Release);
            }
        }
        if pending.is_empty() && (next == trace.len() || report.stop != StopReason::Drained) {
            break;
        }
        let now = elapsed(session.start);
        if report.stop == StopReason::Drained && next < trace.len() && now >= trace[next].at_ns {
            let first_offer = *offered.get_or_insert(now);
            if pending.len() < config.capacity {
                let record = trace[next].clone();
                let id = record.id;
                let admitted = match arrangement {
                    Arrangement::ScatteredJoin => {
                        // The unit queue is sized so a whole admitted record always fits.
                        (0..record.units()).try_fold(true, |_, index| {
                            units
                                .try_send(UnitJob {
                                    record: record.clone(),
                                    position: next,
                                    index,
                                    offered_ns: first_offer,
                                    admitted_ns: now,
                                })
                                .map(|()| true)
                                .map_err(|error| match error {
                                    TrySendError::Full(_) => failed("fan-out unit queue full"),
                                    TrySendError::Disconnected(_) => {
                                        failed("fan-out unit queue disconnected")
                                    }
                                })
                        })?
                    }
                    _ => match records[owner(id, records.len())].try_send(RecordJob {
                        record,
                        position: next,
                        offered_ns: first_offer,
                        admitted_ns: now,
                    }) {
                        Ok(()) => true,
                        Err(TrySendError::Full(_)) => false,
                        Err(TrySendError::Disconnected(_)) => {
                            return Err(failed("fan-out record queue disconnected"));
                        }
                    },
                };
                if admitted {
                    pending.insert(id, next);
                    report.peak_outstanding = report.peak_outstanding.max(pending.len());
                    report.credit_events.push(CreditEvent::Admitted(id));
                    next += 1;
                    offered = None;
                    continue;
                }
                report.credit_full_observations += 1;
            } else {
                report.credit_full_observations += 1;
            }
        }
        let wait = if report.stop == StopReason::Drained
            && next < trace.len()
            && trace[next].at_ns > now
        {
            Duration::from_nanos(trace[next].at_ns - now).min(CHECK_INTERVAL)
        } else {
            CHECK_INTERVAL
        };
        match gathered.recv_timeout(wait) {
            Ok(item) => {
                publication.ready.insert(item.position, item);
                resolve(&mut publication, &mut report, &mut pending)?;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(failed("fan-out results disconnected during wait"));
            }
        }
    }
    report.unadmitted = trace[next..].iter().map(|record| record.id).collect();
    Ok(report)
}

pub fn run(
    config: Config,
    trace: Vec<Record>,
    arrangement: Arrangement,
    external: &AtomicBool,
) -> io::Result<Report> {
    run_with_hooks(
        config,
        trace,
        arrangement,
        external,
        &Hooks {
            service: &service,
            cpu: &cpu_ns,
        },
    )
}

fn run_with_hooks(
    config: Config,
    trace: Vec<Record>,
    arrangement: Arrangement,
    external: &AtomicBool,
    hooks: &Hooks<'_>,
) -> io::Result<Report> {
    validate(&config, &trace)?;
    let computing = unit_threads(&config, arrangement);
    let total = threads(&config, arrangement);
    let cancel = AtomicBool::new(false);
    let abort = AtomicBool::new(false);
    let mut report = thread::scope(|scope| -> io::Result<Report> {
        // One queue per owner, so OwnedJoin's routing is structural rather than incidental.
        let (records, record_receivers): (Vec<_>, Vec<_>) = match arrangement {
            Arrangement::SerialWhole => {
                let (sender, receiver) = bounded::<RecordJob>(config.capacity);
                (vec![sender], vec![receiver])
            }
            Arrangement::OwnedJoin => (0..config.workers)
                .map(|_| bounded::<RecordJob>(config.capacity / config.workers))
                .unzip(),
            Arrangement::ScatteredJoin => (Vec::new(), Vec::new()),
        };
        let unit_capacity = match arrangement {
            Arrangement::ScatteredJoin => config.capacity * MAX_UNITS as usize,
            _ => 1,
        };
        let (units, unit_receiver) = bounded::<UnitJob>(unit_capacity);
        let (outcome_send, outcomes) = bounded::<UnitOutcome>(unit_capacity);
        let (gathered_send, gathered) = bounded::<Gathered>(config.capacity);
        let mut starts = Vec::new();
        let mut handles: Vec<thread::ScopedJoinHandle<'_, io::Result<Produced>>> = Vec::new();
        for index in 0..total {
            let role = role_of(arrangement, index, computing);
            let (gate_send, gate_receive) = bounded(1);
            starts.push(gate_send);
            let record_inbox = record_receivers.get(index).cloned();
            let unit_inbox = unit_receiver.clone();
            let outcome_outbox = outcome_send.clone();
            let outcome_inbox = outcomes.clone();
            let gathered_outbox = gathered_send.clone();
            let cancel = &cancel;
            let abort = &abort;
            let hooks = &*hooks;
            let spawned = thread::Builder::new()
                .name(format!("fanout-{index}"))
                .spawn_scoped(scope, move || {
                    let start = gate_receive
                        .recv()
                        .map_err(|_| failed("fan-out start cancelled"))?;
                    let session = Session {
                        start,
                        deadline: start + Duration::from_millis(config.timeout_ms),
                        cancel,
                        abort,
                    };
                    let mut guard = FailureGuard {
                        session: &session,
                        armed: true,
                    };
                    let result = match role {
                        Role::Owner => {
                            drop(unit_inbox);
                            drop(outcome_outbox);
                            drop(outcome_inbox);
                            let inbox =
                                record_inbox.ok_or_else(|| failed("owner has no record queue"))?;
                            own(index, inbox, gathered_outbox, &session, hooks)
                        }
                        Role::Unit => {
                            drop(record_inbox);
                            drop(outcome_inbox);
                            drop(gathered_outbox);
                            unit_worker(index, unit_inbox, outcome_outbox, &session, hooks)
                        }
                        Role::Joiner => {
                            drop(record_inbox);
                            drop(unit_inbox);
                            drop(outcome_outbox);
                            joiner(index, outcome_inbox, gathered_outbox, hooks)
                        }
                    };
                    guard.armed = result.is_err();
                    result
                });
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    cancel.store(true, Ordering::Release);
                    abort.store(true, Ordering::Release);
                    drop(starts);
                    drop(records);
                    drop(record_receivers);
                    drop(units);
                    drop(unit_receiver);
                    drop(outcome_send);
                    drop(outcomes);
                    drop(gathered_send);
                    drop(gathered);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        drop(record_receivers);
        drop(unit_receiver);
        drop(outcome_send);
        drop(outcomes);
        drop(gathered_send);
        let start = Instant::now();
        let session = Session {
            start,
            deadline: start + Duration::from_millis(config.timeout_ms),
            cancel: &cancel,
            abort: &abort,
        };
        let mut start_error = None;
        for gate in starts {
            if gate.send(start).is_err() {
                start_error = Some(failed("fan-out worker exited before start"));
                break;
            }
        }
        let mut result = match start_error {
            Some(error) => Err(error),
            None => coordinate(
                &config,
                &trace,
                arrangement,
                &Channels {
                    records: &records,
                    units: &units,
                    gathered: &gathered,
                },
                external,
                &session,
            ),
        };
        if result.is_err() {
            abort.store(true, Ordering::Release);
            cancel.store(true, Ordering::Release);
        }
        drop(records);
        drop(units);
        drop(gathered);
        let mut workers = Vec::new();
        let mut peak_records = 0;
        let mut peak_units = 0;
        let mut residual = 0;
        for handle in handles {
            match handle.join() {
                Ok(Ok(produced)) => {
                    peak_records = peak_records.max(produced.peak_partial_records);
                    peak_units = peak_units.max(produced.peak_partial_units);
                    residual += produced.residual_partial_units;
                    workers.push(produced.report);
                }
                Ok(Err(error)) => result = Err(error),
                Err(_) => result = Err(failed("fan-out worker panicked")),
            }
        }
        let mut report = result?;
        workers.sort_by_key(|worker| worker.worker);
        report.workers = workers;
        report.peak_partial_records = peak_records;
        report.peak_partial_units = peak_units;
        report.residual_partial_units = residual;
        report.wall_ns = elapsed(start);
        Ok(report)
    })?;
    report.wall_ns = report.wall_ns.max(
        report
            .completions
            .iter()
            .map(|completion| completion.collected_ns)
            .max()
            .unwrap_or(0),
    );
    verify(&report)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
