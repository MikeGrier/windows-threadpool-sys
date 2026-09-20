// Copyright (c) Mike Grier.
use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

const CHECK_INTERVAL: Duration = Duration::from_millis(1);
const SERVICE_CHUNK: u32 = 256;

struct Job {
    record: Record,
    position: usize,
    offered_ns: u64,
    admitted_ns: u64,
}

enum Staged {
    Ready { value: u64 },
    FailedTransform,
    Interrupted,
}

struct Prepared {
    job: Job,
    worker: usize,
    started_ns: u64,
    ready_ns: u64,
    staged: Staged,
}

struct Session<'a> {
    start: Instant,
    deadline: Instant,
    cancel: &'a AtomicBool,
    abort: &'a AtomicBool,
}

type Service = dyn Fn(&Record, Stage, &AtomicBool, Instant) -> io::Result<bool> + Sync;
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
    stage: Stage,
    cancel: &AtomicBool,
    deadline: Instant,
) -> io::Result<bool> {
    let work = match stage {
        Stage::Transform => record.transform_work,
        Stage::Publish => record.publish_work,
    };
    let mut done = 0;
    let mut value = record.id;
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

fn transform(
    job: Job,
    worker: usize,
    session: &Session<'_>,
    hooks: &Hooks<'_>,
) -> io::Result<Prepared> {
    let started_ns = elapsed(session.start);
    let staged = if job.record.fail_at == Some(Stage::Transform) {
        Staged::FailedTransform
    } else if (hooks.service)(
        &job.record,
        Stage::Transform,
        session.cancel,
        session.deadline,
    )? {
        Staged::Ready {
            value: reference(&job.record),
        }
    } else {
        Staged::Interrupted
    };
    Ok(Prepared {
        worker,
        started_ns,
        ready_ns: elapsed(session.start),
        staged,
        job,
    })
}

/// The publication side, authoritative for every terminal outcome. Once it observes
/// cancellation it cancels the cursor record and every record after it, so the
/// outcome sequence is a published/aborted prefix and an all-cancelled suffix.
#[derive(Default)]
struct Publication {
    cancelling: bool,
    log: Vec<Published>,
    published: usize,
    aborted: usize,
    cancelled: usize,
    active_ns: u64,
}

impl Publication {
    fn resolve(
        &mut self,
        prepared: Prepared,
        session: &Session<'_>,
        hooks: &Hooks<'_>,
    ) -> io::Result<Completion> {
        let record = &prepared.job.record;
        let published_ns = elapsed(session.start);
        let outcome = if self.cancelling {
            Outcome::Cancelled
        } else {
            match prepared.staged {
                Staged::Interrupted => {
                    self.cancelling = true;
                    Outcome::Cancelled
                }
                Staged::FailedTransform => Outcome::Aborted {
                    stage: Stage::Transform,
                },
                Staged::Ready { value } => {
                    if session.cancel.load(Ordering::Acquire) || Instant::now() >= session.deadline
                    {
                        self.cancelling = true;
                        Outcome::Cancelled
                    } else if record.fail_at == Some(Stage::Publish) {
                        Outcome::Aborted {
                            stage: Stage::Publish,
                        }
                    } else if (hooks.service)(
                        record,
                        Stage::Publish,
                        session.cancel,
                        session.deadline,
                    )? {
                        self.log.push(Published {
                            id: record.id,
                            value,
                        });
                        Outcome::Published { value }
                    } else {
                        self.cancelling = true;
                        Outcome::Cancelled
                    }
                }
            }
        };
        let finished_ns = elapsed(session.start);
        self.active_ns += finished_ns - published_ns;
        match outcome {
            Outcome::Published { .. } => self.published += 1,
            Outcome::Aborted { .. } => self.aborted += 1,
            Outcome::Cancelled => self.cancelled += 1,
        }
        Ok(Completion {
            id: record.id,
            position: prepared.job.position,
            worker: prepared.worker,
            scheduled_ns: record.at_ns,
            offered_ns: prepared.job.offered_ns,
            admitted_ns: prepared.job.admitted_ns,
            started_ns: prepared.started_ns,
            ready_ns: prepared.ready_ns,
            published_ns,
            finished_ns,
            collected_ns: 0,
            outcome,
        })
    }

    fn report(&self, worker: usize, role: Role, transformed: usize, cpu_ns: u64) -> WorkerReport {
        WorkerReport {
            worker,
            role,
            transformed,
            published: self.published,
            aborted: self.aborted,
            cancelled: self.cancelled,
            active_ns: self.active_ns,
            cpu_ns,
        }
    }
}

struct ReorderState {
    slots: BTreeMap<usize, Prepared>,
    cursor: usize,
    live: usize,
    peak: usize,
    full: usize,
}

struct Reorder {
    state: Mutex<ReorderState>,
    space: Condvar,
    ready: Condvar,
}

impl Reorder {
    fn new(live: usize) -> Self {
        Self {
            state: Mutex::new(ReorderState {
                slots: BTreeMap::new(),
                cursor: 0,
                live,
                peak: 0,
                full: 0,
            }),
            space: Condvar::new(),
            ready: Condvar::new(),
        }
    }

    /// The window is `capacity` positions starting at the cursor, so the record the
    /// publisher is waiting for can always be inserted and the pipeline cannot
    /// deadlock against its own bound.
    fn stage(&self, prepared: Prepared, capacity: usize, session: &Session<'_>) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| failed("reorder buffer poisoned"))?;
        let mut blocked = false;
        while prepared.job.position >= state.cursor + capacity {
            if session.abort.load(Ordering::Acquire) {
                return Err(failed("peer failed while waiting for publication order"));
            }
            if !blocked {
                state.full += 1;
                blocked = true;
            }
            state = self
                .space
                .wait_timeout(state, CHECK_INTERVAL)
                .map_err(|_| failed("reorder space wait poisoned"))?
                .0;
        }
        state.slots.insert(prepared.job.position, prepared);
        state.peak = state.peak.max(state.slots.len());
        self.ready.notify_all();
        Ok(())
    }

    fn retire(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.live -= 1;
        }
        self.ready.notify_all();
    }

    fn take(&self, session: &Session<'_>) -> io::Result<Option<Prepared>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| failed("reorder buffer poisoned"))?;
        loop {
            if session.abort.load(Ordering::Acquire) {
                return Err(failed("peer failed before publication"));
            }
            let cursor = state.cursor;
            if let Some(prepared) = state.slots.remove(&cursor) {
                state.cursor += 1;
                self.space.notify_all();
                return Ok(Some(prepared));
            }
            if state.live == 0 {
                if state.slots.is_empty() {
                    return Ok(None);
                }
                return Err(failed("staged record never reached the publisher"));
            }
            state = self
                .ready
                .wait_timeout(state, CHECK_INTERVAL)
                .map_err(|_| failed("reorder ready wait poisoned"))?
                .0;
        }
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
    reports: Vec<WorkerReport>,
    log: Vec<Published>,
}

fn owner(
    requests: Receiver<Job>,
    completions: Sender<Completion>,
    session: &Session<'_>,
    hooks: &Hooks<'_>,
) -> io::Result<Produced> {
    let initial = (hooks.cpu)()?;
    let mut publication = Publication::default();
    let mut transformed = 0;
    let mut transform_active = 0;
    while let Ok(job) = requests.recv() {
        if session.abort.load(Ordering::Acquire) {
            return Err(failed("peer failed"));
        }
        let prepared = transform(job, 0, session, hooks)?;
        transformed += 1;
        transform_active += prepared.ready_ns - prepared.started_ns;
        let completion = publication.resolve(prepared, session, hooks)?;
        completions
            .send(completion)
            .map_err(|_| failed("ingestion collector disconnected"))?;
    }
    let cpu = (hooks.cpu)()?.saturating_sub(initial);
    let mut report = publication.report(0, Role::Owner, transformed, cpu);
    report.active_ns += transform_active;
    Ok(Produced {
        reports: vec![report],
        log: publication.log,
    })
}

fn transformer(
    index: usize,
    requests: Receiver<Job>,
    reorder: &Reorder,
    capacity: usize,
    session: &Session<'_>,
    hooks: &Hooks<'_>,
) -> io::Result<WorkerReport> {
    let initial = (hooks.cpu)()?;
    let mut transformed = 0;
    let mut active_ns = 0;
    while let Ok(job) = requests.recv() {
        if session.abort.load(Ordering::Acquire) {
            return Err(failed("peer failed"));
        }
        let prepared = transform(job, index, session, hooks)?;
        transformed += 1;
        active_ns += prepared.ready_ns - prepared.started_ns;
        reorder.stage(prepared, capacity, session)?;
    }
    Ok(WorkerReport {
        worker: index,
        role: Role::Transform,
        transformed,
        published: 0,
        aborted: 0,
        cancelled: 0,
        active_ns,
        cpu_ns: (hooks.cpu)()?.saturating_sub(initial),
    })
}

fn publisher(
    index: usize,
    reorder: &Reorder,
    completions: Sender<Completion>,
    session: &Session<'_>,
    hooks: &Hooks<'_>,
) -> io::Result<Produced> {
    let initial = (hooks.cpu)()?;
    let mut publication = Publication::default();
    while let Some(prepared) = reorder.take(session)? {
        let completion = publication.resolve(prepared, session, hooks)?;
        completions
            .send(completion)
            .map_err(|_| failed("ingestion collector disconnected"))?;
    }
    let cpu = (hooks.cpu)()?.saturating_sub(initial);
    Ok(Produced {
        reports: vec![publication.report(index, Role::Publisher, 0, cpu)],
        log: publication.log,
    })
}

fn collect(
    mut completion: Completion,
    report: &mut Report,
    pending: &mut BTreeMap<u64, usize>,
    session: &Session<'_>,
) -> io::Result<()> {
    if pending.remove(&completion.id) != Some(completion.position) {
        return Err(failed("ingestion completion correlation mismatch"));
    }
    completion.collected_ns = elapsed(session.start);
    report
        .credit_events
        .push(CreditEvent::Collected(completion.id));
    report.completions.push(completion);
    Ok(())
}

fn coordinate(
    config: &Config,
    trace: &[Record],
    arrangement: Arrangement,
    requests: &Sender<Job>,
    completions: &Receiver<Completion>,
    external: &AtomicBool,
    session: &Session<'_>,
) -> io::Result<Report> {
    let limits = &config.limits;
    let mut report = Report {
        schema: "ingest-v1".into(),
        evidence_class: "offline_unpinned_ordered_ingestion".into(),
        placement: "unpinned_host_threads_no_numa_claim".into(),
        config: config.clone(),
        arrangement,
        stop: StopReason::Drained,
        wall_ns: 0,
        request_slots: requests.capacity().unwrap(),
        reply_slots: completions.capacity().unwrap(),
        reorder_slots: reorder_slots(config, arrangement),
        peak_outstanding: 0,
        peak_reorder_occupancy: 0,
        credit_full_observations: 0,
        reorder_full_observations: 0,
        trace: trace.to_vec(),
        completions: Vec::with_capacity(trace.len()),
        unadmitted: vec![],
        credit_events: Vec::with_capacity(trace.len() * 2),
        workers: vec![],
        log: vec![],
    };
    let mut pending = BTreeMap::new();
    let mut next = 0;
    let mut offered = None;
    while next < trace.len() || !pending.is_empty() {
        if session.abort.load(Ordering::Acquire) {
            return Err(failed("ingestion worker failed"));
        }
        loop {
            match completions.try_recv() {
                Ok(completion) => collect(completion, &mut report, &mut pending, session)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(failed("ingestion completions disconnected"));
                }
            }
        }
        if report.stop == StopReason::Drained {
            if external.load(Ordering::Acquire)
                || limits
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
            if pending.len() < limits.capacity {
                let record = trace[next].clone();
                let id = record.id;
                match requests.try_send(Job {
                    record,
                    position: next,
                    offered_ns: first_offer,
                    admitted_ns: now,
                }) {
                    Ok(()) => {
                        pending.insert(id, next);
                        report.peak_outstanding = report.peak_outstanding.max(pending.len());
                        report.credit_events.push(CreditEvent::Admitted(id));
                        next += 1;
                        offered = None;
                        continue;
                    }
                    Err(TrySendError::Full(_)) => report.credit_full_observations += 1,
                    Err(TrySendError::Disconnected(_)) => {
                        return Err(failed("ingestion request queue disconnected"));
                    }
                }
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
        match completions.recv_timeout(wait) {
            Ok(completion) => collect(completion, &mut report, &mut pending, session)?,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(failed("ingestion completions disconnected during wait"));
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
    let limits = &config.limits;
    let count = transformers(&config, arrangement);
    let reorder = Reorder::new(count);
    let cancel = AtomicBool::new(false);
    let abort = AtomicBool::new(false);
    let mut report = thread::scope(|scope| -> io::Result<Report> {
        let (requests, receiver) = bounded::<Job>(limits.capacity);
        let (completion_send, completions) = bounded::<Completion>(limits.capacity);
        let mut starts = Vec::new();
        let mut handles: Vec<thread::ScopedJoinHandle<'_, io::Result<Produced>>> = Vec::new();
        let roles = match arrangement {
            Arrangement::SerialOwner => vec![Role::Owner],
            Arrangement::StagedPipeline => (0..count)
                .map(|_| Role::Transform)
                .chain([Role::Publisher])
                .collect(),
        };
        for (index, role) in roles.into_iter().enumerate() {
            let (gate_send, gate_receive) = bounded(1);
            starts.push(gate_send);
            let inbox = receiver.clone();
            let outbox = completion_send.clone();
            let reorder = &reorder;
            let cancel = &cancel;
            let abort = &abort;
            let spawned = thread::Builder::new()
                .name(format!("ingest-{index}"))
                .spawn_scoped(scope, move || {
                    let start = gate_receive
                        .recv()
                        .map_err(|_| failed("ingestion start cancelled"))?;
                    let session = Session {
                        start,
                        deadline: start + Duration::from_millis(limits.timeout_ms),
                        cancel,
                        abort,
                    };
                    let mut guard = FailureGuard {
                        session: &session,
                        armed: true,
                    };
                    let result = match role {
                        Role::Owner => owner(inbox, outbox, &session, hooks),
                        Role::Transform => {
                            drop(outbox);
                            let produced = transformer(
                                index,
                                inbox,
                                reorder,
                                config.reorder_capacity,
                                &session,
                                hooks,
                            )
                            .map(|report| Produced {
                                reports: vec![report],
                                log: vec![],
                            });
                            reorder.retire();
                            produced
                        }
                        Role::Publisher => {
                            drop(inbox);
                            publisher(index, reorder, outbox, &session, hooks)
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
                    drop(requests);
                    drop(receiver);
                    drop(completion_send);
                    drop(completions);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        drop(receiver);
        drop(completion_send);
        let start = Instant::now();
        let session = Session {
            start,
            deadline: start + Duration::from_millis(limits.timeout_ms),
            cancel: &cancel,
            abort: &abort,
        };
        let mut start_error = None;
        for gate in starts {
            if gate.send(start).is_err() {
                start_error = Some(failed("ingestion worker exited before start"));
                break;
            }
        }
        let mut result = match start_error {
            Some(error) => Err(error),
            None => coordinate(
                &config,
                &trace,
                arrangement,
                &requests,
                &completions,
                external,
                &session,
            ),
        };
        if result.is_err() {
            abort.store(true, Ordering::Release);
            cancel.store(true, Ordering::Release);
        }
        drop(requests);
        drop(completions);
        let mut workers = Vec::new();
        let mut log = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(produced)) => {
                    workers.extend(produced.reports);
                    if !produced.log.is_empty() {
                        log = produced.log;
                    }
                }
                Ok(Err(error)) => result = Err(error),
                Err(_) => result = Err(failed("ingestion worker panicked")),
            }
        }
        let mut report = result?;
        workers.sort_by_key(|worker| worker.worker);
        report.workers = workers;
        report.log = log;
        report.wall_ns = elapsed(start);
        Ok(report)
    })?;
    let state = reorder
        .state
        .into_inner()
        .map_err(|_| failed("reorder buffer poisoned"))?;
    report.peak_reorder_occupancy = state.peak;
    report.reorder_full_observations = state.full;
    verify(&report)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
