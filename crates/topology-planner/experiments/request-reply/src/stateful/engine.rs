// Copyright (c) Mike Grier.
use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

const CHECK_INTERVAL: Duration = Duration::from_millis(1);
const SERVICE_CHUNK: u32 = 256;

#[derive(Default)]
struct Slot {
    value: u64,
    next: usize,
    cancelled: bool,
}

struct Shared {
    slots: Mutex<Vec<Slot>>,
    changed: Condvar,
}
enum State {
    Shared(Arc<Shared>),
    Owned(Vec<(usize, Slot)>),
}
struct Job {
    request: Request,
    sequence: usize,
    offered_ns: u64,
    admitted_ns: u64,
}
struct Session<'a> {
    start: Instant,
    deadline: Instant,
    cancel: &'a AtomicBool,
    abort: &'a AtomicBool,
}
type Prepare = dyn Fn(&Request, &AtomicBool, Instant) -> io::Result<bool> + Sync;
struct Hooks<'a> {
    prepare: &'a Prepare,
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

fn prepare(request: &Request, cancel: &AtomicBool, deadline: Instant) -> io::Result<bool> {
    let mut done = 0;
    let mut value = request.id;
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Ok(false);
        }
        if done == request.work {
            return Ok(true);
        }
        let end = request.work.min(done + SERVICE_CHUNK);
        for step in done..end {
            value = std::hint::black_box(value.wrapping_add(u64::from(step)));
        }
        done = end;
    }
}

fn apply(
    slot: &mut Slot,
    job: &Job,
    ready: bool,
    session: &Session<'_>,
) -> io::Result<(Outcome, u64)> {
    if slot.next != job.sequence {
        return Err(failed("key sequence does not match next commit"));
    }
    let outcome = if !ready
        || slot.cancelled
        || session.cancel.load(Ordering::Acquire)
        || Instant::now() >= session.deadline
    {
        slot.cancelled = true;
        Outcome::Cancelled
    } else {
        match job.request.operation {
            Operation::Lookup => {}
            Operation::Add { value } => slot.value = slot.value.wrapping_add(value),
        }
        Outcome::Completed { value: slot.value }
    };
    let committed = elapsed(session.start);
    slot.next += 1;
    Ok((outcome, committed))
}

impl State {
    fn commit(
        &mut self,
        job: &Job,
        ready: bool,
        workers: usize,
        session: &Session<'_>,
    ) -> io::Result<(Outcome, u64)> {
        match self {
            State::Shared(shared) => {
                let mut slots = shared
                    .slots
                    .lock()
                    .map_err(|_| failed("shared state poisoned"))?;
                while slots[job.request.key].next != job.sequence {
                    if session.abort.load(Ordering::Acquire) {
                        return Err(failed("peer failed during key-order wait"));
                    }
                    slots = shared
                        .changed
                        .wait_timeout(slots, CHECK_INTERVAL)
                        .map_err(|_| failed("shared order wait poisoned"))?
                        .0;
                }
                if session.abort.load(Ordering::Acquire) {
                    return Err(failed("peer failed before commit"));
                }
                let result = apply(&mut slots[job.request.key], job, ready, session);
                shared.changed.notify_all();
                result
            }
            State::Owned(partition) => {
                let (key, slot) = partition
                    .get_mut(job.request.key / workers)
                    .ok_or_else(|| failed("key absent from owner partition"))?;
                if *key != job.request.key {
                    return Err(failed("request reached wrong key owner"));
                }
                apply(slot, job, ready, session)
            }
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

struct WorkerResult {
    report: WorkerReport,
    values: Vec<(usize, u64)>,
}

fn worker(
    index: usize,
    workers: usize,
    requests: Receiver<Job>,
    replies: Sender<Reply>,
    mut state: State,
    session: &Session<'_>,
    hooks: &Hooks<'_>,
) -> io::Result<WorkerResult> {
    let initial_cpu = (hooks.cpu)()?;
    let mut report = WorkerReport {
        worker: index,
        completed: 0,
        cancelled: 0,
        active_ns: 0,
        cpu_ns: 0,
    };
    while let Ok(job) = requests.recv() {
        if session.abort.load(Ordering::Acquire) {
            return Err(failed("peer failed"));
        }
        let started_ns = elapsed(session.start);
        let ready = (hooks.prepare)(&job.request, session.cancel, session.deadline)?;
        let (outcome, committed_ns) = state.commit(&job, ready, workers, session)?;
        let finished_ns = elapsed(session.start);
        report.active_ns += finished_ns - started_ns;
        match outcome {
            Outcome::Completed { .. } => report.completed += 1,
            Outcome::Cancelled => report.cancelled += 1,
        }
        replies
            .send(Reply {
                id: job.request.id,
                key: job.request.key,
                sequence: job.sequence,
                worker: index,
                scheduled_ns: job.request.at_ns,
                offered_ns: job.offered_ns,
                admitted_ns: job.admitted_ns,
                started_ns,
                committed_ns,
                finished_ns,
                collected_ns: 0,
                outcome,
            })
            .map_err(|_| failed("stateful collector disconnected"))?;
    }
    report.cpu_ns = (hooks.cpu)()?.saturating_sub(initial_cpu);
    let values = match state {
        State::Owned(partition) => partition
            .into_iter()
            .map(|(key, slot)| (key, slot.value))
            .collect(),
        State::Shared(_) => Vec::new(),
    };
    Ok(WorkerResult { report, values })
}

fn collect(
    mut reply: Reply,
    report: &mut Report,
    pending: &mut BTreeMap<u64, usize>,
    depths: &mut [usize],
    session: &Session<'_>,
) -> io::Result<()> {
    if pending.remove(&reply.id) != Some(reply.key) {
        return Err(failed("stateful reply correlation mismatch"));
    }
    depths[reply.key] -= 1;
    reply.collected_ns = elapsed(session.start);
    report.credit_events.push(CreditEvent::Collected(reply.id));
    report.replies.push(reply);
    Ok(())
}

fn coordinate(
    config: &Config,
    trace: &[Request],
    arrangement: Arrangement,
    senders: &[Sender<Job>],
    replies: &Receiver<Reply>,
    external: &AtomicBool,
    session: &Session<'_>,
) -> io::Result<Report> {
    let limits = &config.limits;
    let mut report = Report {
        schema: "stateful-v1".into(),
        evidence_class: "offline_unpinned_key_state_service".into(),
        placement: "unpinned_host_threads_no_numa_claim".into(),
        config: config.clone(),
        arrangement,
        stop: StopReason::Drained,
        wall_ns: 0,
        request_slots: senders
            .iter()
            .map(|sender| sender.capacity().unwrap())
            .sum(),
        reply_slots: replies.capacity().unwrap(),
        state_entries: config.keys,
        peak_outstanding: 0,
        key_peaks: vec![0; config.keys],
        lane_peaks: vec![0; limits.workers],
        credit_full_observations: 0,
        lane_full_observations: vec![0; limits.workers],
        trace: trace.to_vec(),
        replies: Vec::with_capacity(trace.len()),
        unadmitted: vec![],
        credit_events: Vec::with_capacity(trace.len() * 2),
        workers: vec![],
        final_state: vec![0; config.keys],
    };
    let mut pending = BTreeMap::new();
    let mut depths = vec![0; config.keys];
    let mut sequences = vec![0; config.keys];
    let mut next = 0;
    let mut offered = None;
    while next < trace.len() || !pending.is_empty() {
        if session.abort.load(Ordering::Acquire) {
            return Err(failed("stateful worker failed"));
        }
        loop {
            match replies.try_recv() {
                Ok(reply) => collect(reply, &mut report, &mut pending, &mut depths, session)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(failed("stateful replies disconnected"));
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
                let request = trace[next].clone();
                let key = request.key;
                let id = request.id;
                let lane = owner(key, limits.workers);
                let target = match arrangement {
                    Arrangement::SharedState => 0,
                    Arrangement::KeyOwned => lane,
                };
                match senders[target].try_send(Job {
                    request,
                    sequence: sequences[key],
                    offered_ns: first_offer,
                    admitted_ns: now,
                }) {
                    Ok(()) => {
                        pending.insert(id, key);
                        depths[key] += 1;
                        sequences[key] += 1;
                        report.key_peaks[key] = report.key_peaks[key].max(depths[key]);
                        let lane_depth = pending
                            .values()
                            .filter(|&&key| owner(key, limits.workers) == lane)
                            .count();
                        report.lane_peaks[lane] = report.lane_peaks[lane].max(lane_depth);
                        report.peak_outstanding = report.peak_outstanding.max(pending.len());
                        report.credit_events.push(CreditEvent::Admitted(id));
                        next += 1;
                        offered = None;
                        continue;
                    }
                    Err(TrySendError::Full(_)) => report.lane_full_observations[lane] += 1,
                    Err(TrySendError::Disconnected(_)) => {
                        return Err(failed("stateful request lane disconnected"));
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
        match replies.recv_timeout(wait) {
            Ok(reply) => collect(reply, &mut report, &mut pending, &mut depths, session)?,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(failed("stateful replies disconnected during wait"));
            }
        }
    }
    report.unadmitted = trace[next..].iter().map(|request| request.id).collect();
    Ok(report)
}

pub fn run(
    config: Config,
    trace: Vec<Request>,
    arrangement: Arrangement,
    external: &AtomicBool,
) -> io::Result<Report> {
    run_with_hooks(
        config,
        trace,
        arrangement,
        external,
        &Hooks {
            prepare: &prepare,
            cpu: &cpu_ns,
        },
    )
}

fn run_with_hooks(
    config: Config,
    trace: Vec<Request>,
    arrangement: Arrangement,
    external: &AtomicBool,
    hooks: &Hooks<'_>,
) -> io::Result<Report> {
    validate(&config, &trace)?;
    let limits = &config.limits;
    let shared = Arc::new(Shared {
        slots: Mutex::new(if arrangement == Arrangement::SharedState {
            (0..config.keys).map(|_| Slot::default()).collect()
        } else {
            Vec::new()
        }),
        changed: Condvar::new(),
    });
    let (senders, receivers): (Vec<_>, Vec<_>) = match arrangement {
        Arrangement::SharedState => {
            let (sender, receiver) = bounded(limits.capacity);
            (vec![sender], vec![receiver; limits.workers])
        }
        Arrangement::KeyOwned => (0..limits.workers)
            .map(|_| bounded(limits.capacity / limits.workers))
            .unzip(),
    };
    let cancel = AtomicBool::new(false);
    let abort = AtomicBool::new(false);
    let mut report = thread::scope(|scope| -> io::Result<Report> {
        let (reply_send, replies) = bounded(limits.capacity);
        let mut starts = Vec::new();
        let mut handles: Vec<thread::ScopedJoinHandle<'_, io::Result<WorkerResult>>> = Vec::new();
        for (index, requests) in receivers.into_iter().enumerate() {
            let state = match arrangement {
                Arrangement::SharedState => State::Shared(Arc::clone(&shared)),
                Arrangement::KeyOwned => State::Owned(
                    (index..config.keys)
                        .step_by(limits.workers)
                        .map(|key| (key, Slot::default()))
                        .collect(),
                ),
            };
            let (gate_send, gate_receive) = bounded(1);
            starts.push(gate_send);
            let reply_send = reply_send.clone();
            let cancel = &cancel;
            let abort = &abort;
            let spawned = thread::Builder::new()
                .name(format!("key-service-{index}"))
                .spawn_scoped(scope, move || {
                    let start = gate_receive
                        .recv()
                        .map_err(|_| failed("stateful start cancelled"))?;
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
                    let result = worker(
                        index,
                        limits.workers,
                        requests,
                        reply_send,
                        state,
                        &session,
                        hooks,
                    );
                    guard.armed = result.is_err();
                    result
                });
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    cancel.store(true, Ordering::Release);
                    drop(starts);
                    drop(senders);
                    drop(replies);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        drop(reply_send);
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
                start_error = Some(failed("stateful worker exited before start"));
                break;
            }
        }
        let mut result = match start_error {
            Some(error) => Err(error),
            None => coordinate(
                &config,
                &trace,
                arrangement,
                &senders,
                &replies,
                external,
                &session,
            ),
        };
        if result.is_err() {
            abort.store(true, Ordering::Release);
            cancel.store(true, Ordering::Release);
        }
        drop(senders);
        drop(replies);
        let mut workers = Vec::new();
        let mut final_state = vec![0; config.keys];
        for handle in handles {
            match handle.join() {
                Ok(Ok(result)) => {
                    for (key, value) in result.values {
                        final_state[key] = value;
                    }
                    workers.push(result.report);
                }
                Ok(Err(error)) => result = Err(error),
                Err(_) => result = Err(failed("stateful worker panicked")),
            }
        }
        let mut report = result?;
        if arrangement == Arrangement::SharedState {
            final_state = shared
                .slots
                .lock()
                .map_err(|_| failed("shared final state poisoned"))?
                .iter()
                .map(|slot| slot.value)
                .collect();
        }
        report.final_state = final_state;
        report.workers = workers;
        report.wall_ns = elapsed(start);
        Ok(report)
    })?;
    report.replies.sort_by_key(|reply| reply.id);
    verify(&report)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
