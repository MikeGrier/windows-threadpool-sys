// Copyright (c) Mike Grier.
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};

use crate::{
    Arrangement, Config, CreditEvent, Outcome, Reply, Report, Request, StopReason, WorkerReport,
    failed, validate, verify,
};

const CANCEL_POLL: Duration = Duration::from_millis(1);
const COMPUTE_CHUNK: u32 = 256;

struct Job {
    request: Request,
    offered_ns: u64,
    admitted_ns: u64,
}

struct Queues {
    senders: Vec<Sender<Job>>,
    receivers: Vec<Receiver<Job>>,
}

fn shared_service(config: &Config) -> Queues {
    let (send, receive) = bounded(config.capacity);
    Queues {
        senders: vec![send],
        receivers: vec![receive; config.workers],
    }
}

fn assigned_service(config: &Config) -> Queues {
    let (senders, receivers) = (0..config.workers)
        .map(|_| bounded(config.capacity / config.workers))
        .unzip();
    Queues { senders, receivers }
}

fn elapsed(start: Instant) -> u64 {
    start.elapsed().as_nanos() as u64
}

fn service(request: &Request, cancel: &AtomicBool, deadline: Instant) -> Outcome {
    let mut value = request.value;
    let mut done = 0;
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Outcome::Cancelled;
        }
        if done == request.work {
            return Outcome::Completed { value };
        }
        let end = request.work.min(done + COMPUTE_CHUNK);
        for step in done + 1..=end {
            value = std::hint::black_box(value.wrapping_add(u64::from(step)));
        }
        done = end;
    }
}

type Processor = dyn Fn(&Request, &AtomicBool, Instant) -> Outcome + Sync;

struct FailureGuard<'a> {
    abort: &'a AtomicBool,
    cancel: &'a AtomicBool,
    armed: bool,
}
impl Drop for FailureGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.abort.store(true, Ordering::Release);
            self.cancel.store(true, Ordering::Release);
        }
    }
}

fn worker(
    worker: usize,
    requests: Receiver<Job>,
    replies: Sender<Reply>,
    start: Instant,
    timeout: Duration,
    cancel: &AtomicBool,
    process: &Processor,
) -> io::Result<WorkerReport> {
    let deadline = start + timeout;
    let mut stats = WorkerReport {
        worker,
        handled: 0,
        completed: 0,
        cancelled: 0,
        active_ns: 0,
    };
    while let Ok(job) = requests.recv() {
        let started_ns = elapsed(start);
        let outcome = process(&job.request, cancel, deadline);
        let finished_ns = elapsed(start);
        stats.handled += 1;
        stats.active_ns += finished_ns - started_ns;
        match outcome {
            Outcome::Completed { .. } => stats.completed += 1,
            Outcome::Cancelled => stats.cancelled += 1,
        }
        replies
            .send(Reply {
                id: job.request.id,
                lane: job.request.lane,
                worker,
                scheduled_ns: job.request.at_ns,
                offered_ns: job.offered_ns,
                admitted_ns: job.admitted_ns,
                started_ns,
                finished_ns,
                collected_ns: 0,
                outcome,
            })
            .map_err(|_| failed("reply collector disconnected"))?;
    }
    Ok(stats)
}

fn collect(
    mut reply: Reply,
    report: &mut Report,
    pending: &mut std::collections::BTreeSet<u64>,
    lane_depths: &mut [usize],
    start: Instant,
) -> io::Result<()> {
    if !pending.remove(&reply.id) || reply.lane >= lane_depths.len() || lane_depths[reply.lane] == 0
    {
        return Err(failed("unexpected or duplicate terminal reply"));
    }
    lane_depths[reply.lane] -= 1;
    reply.collected_ns = elapsed(start);
    report.credit_events.push(CreditEvent::Collected(reply.id));
    report.replies.push(reply);
    Ok(())
}

struct Session<'a> {
    start: Instant,
    external_cancel: &'a AtomicBool,
    cancel: &'a AtomicBool,
    abort: &'a AtomicBool,
}

fn coordinate(
    config: &Config,
    trace: &[Request],
    arrangement: Arrangement,
    senders: &[Sender<Job>],
    replies: &Receiver<Reply>,
    session: &Session<'_>,
) -> io::Result<Report> {
    let Session {
        start,
        external_cancel,
        cancel,
        abort,
    } = *session;
    let mut report = Report {
        schema: "request-reply-v1".into(),
        evidence_class: "offline_unpinned_in_memory_request_service".into(),
        config: config.clone(),
        arrangement,
        stop: StopReason::Drained,
        wall_ns: 0,
        request_slots: senders
            .iter()
            .map(|sender| sender.capacity().unwrap())
            .sum(),
        reply_slots: config.capacity,
        peak_outstanding: 0,
        lane_peaks: vec![0; config.workers],
        credit_full_observations: 0,
        lane_full_observations: vec![0; config.workers],
        trace: trace.to_vec(),
        replies: Vec::with_capacity(trace.len()),
        unadmitted: Vec::new(),
        credit_events: Vec::with_capacity(trace.len() * 2),
        workers: Vec::new(),
    };
    let deadline = start + Duration::from_millis(config.timeout_ms);
    let mut pending = std::collections::BTreeSet::new();
    let mut lane_depths = vec![0; config.workers];
    let mut next = 0;
    let mut offered_ns = None;
    while next < trace.len() || !pending.is_empty() {
        if abort.load(Ordering::Acquire) {
            return Err(failed("request worker failed"));
        }
        loop {
            match replies.try_recv() {
                Ok(reply) => collect(reply, &mut report, &mut pending, &mut lane_depths, start)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(failed("all reply workers disconnected"));
                }
            }
        }
        if report.stop == StopReason::Drained {
            if external_cancel.load(Ordering::Acquire)
                || config
                    .cancel_after_admitted
                    .is_some_and(|count| next >= count)
            {
                report.stop = StopReason::Cancelled;
            } else if Instant::now() >= deadline {
                report.stop = StopReason::Deadline;
            }
            if report.stop != StopReason::Drained {
                cancel.store(true, Ordering::Release);
            }
        }
        if report.stop != StopReason::Drained && pending.is_empty() {
            break;
        }
        if next == trace.len() && pending.is_empty() {
            break;
        }
        let now = elapsed(start);
        if report.stop == StopReason::Drained && next < trace.len() && now >= trace[next].at_ns {
            let offered = *offered_ns.get_or_insert(now);
            if pending.len() < config.capacity {
                let request = trace[next].clone();
                let target = match arrangement {
                    Arrangement::Shared => 0,
                    Arrangement::Assigned => request.lane,
                };
                let id = request.id;
                let lane = request.lane;
                match senders[target].try_send(Job {
                    request,
                    offered_ns: offered,
                    admitted_ns: now,
                }) {
                    Ok(()) => {
                        pending.insert(id);
                        lane_depths[lane] += 1;
                        report.peak_outstanding = report.peak_outstanding.max(pending.len());
                        report.lane_peaks[lane] = report.lane_peaks[lane].max(lane_depths[lane]);
                        report.credit_events.push(CreditEvent::Admitted(id));
                        next += 1;
                        offered_ns = None;
                        continue;
                    }
                    Err(TrySendError::Full(_)) => report.lane_full_observations[lane] += 1,
                    Err(TrySendError::Disconnected(_)) => {
                        return Err(failed("request receiver disconnected"));
                    }
                }
            } else {
                report.credit_full_observations += 1;
            }
        }
        let until_arrival = if report.stop == StopReason::Drained
            && next < trace.len()
            && trace[next].at_ns > now
        {
            Duration::from_nanos(trace[next].at_ns - now)
        } else {
            CANCEL_POLL
        };
        let wait = until_arrival.min(CANCEL_POLL);
        match replies.recv_timeout(wait) {
            Ok(reply) => collect(reply, &mut report, &mut pending, &mut lane_depths, start)?,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(failed("reply workers disconnected during wait"));
            }
        }
    }
    report.unadmitted = trace[next..].iter().map(|request| request.id).collect();
    report.wall_ns = elapsed(start);
    Ok(report)
}

pub fn run(
    config: Config,
    trace: Vec<Request>,
    arrangement: Arrangement,
    external_cancel: &AtomicBool,
) -> io::Result<Report> {
    run_with_processor(config, trace, arrangement, external_cancel, &service)
}

fn run_with_processor(
    config: Config,
    trace: Vec<Request>,
    arrangement: Arrangement,
    external_cancel: &AtomicBool,
    process: &Processor,
) -> io::Result<Report> {
    validate(&config, &trace)?;
    let queues = match arrangement {
        Arrangement::Shared => shared_service(&config),
        Arrangement::Assigned => assigned_service(&config),
    };
    let cancel = AtomicBool::new(false);
    let abort = AtomicBool::new(false);
    let mut report = thread::scope(|scope| -> io::Result<Report> {
        let (reply_send, replies) = bounded(config.capacity);
        let mut starts = Vec::new();
        let mut handles: Vec<thread::ScopedJoinHandle<'_, io::Result<WorkerReport>>> = Vec::new();
        for (index, requests) in queues.receivers.into_iter().enumerate() {
            let (send, gate) = bounded(1);
            starts.push(send);
            let reply_send = reply_send.clone();
            let cancel = &cancel;
            let abort = &abort;
            let timeout = Duration::from_millis(config.timeout_ms);
            let spawned = thread::Builder::new()
                .name(format!("request-reply-{index}"))
                .spawn_scoped(scope, move || {
                    let mut guard = FailureGuard {
                        cancel,
                        abort,
                        armed: true,
                    };
                    let start = gate
                        .recv()
                        .map_err(|_| failed("start coordinator disconnected"))?;
                    let result =
                        worker(index, requests, reply_send, start, timeout, cancel, process);
                    guard.armed = result.is_err();
                    result
                });
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    cancel.store(true, Ordering::Release);
                    drop(starts);
                    drop(queues.senders);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        drop(reply_send);
        let start = Instant::now();
        let mut start_error = None;
        for gate in starts {
            if gate.send(start).is_err() {
                start_error = Some(failed("worker exited before start"));
                break;
            }
        }
        let session = Session {
            start,
            external_cancel,
            cancel: &cancel,
            abort: &abort,
        };
        let mut result = match start_error {
            Some(error) => Err(error),
            None => coordinate(
                &config,
                &trace,
                arrangement,
                &queues.senders,
                &replies,
                &session,
            ),
        };
        if result.is_err() {
            cancel.store(true, Ordering::Release);
        }
        drop(queues.senders);
        let mut workers = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(stats)) => workers.push(stats),
                Ok(Err(error)) => result = Err(error),
                Err(_) => result = Err(failed("request worker panicked")),
            }
        }
        let mut report = result?;
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
