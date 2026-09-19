// Copyright (c) Mike Grier.
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use windows_overlapped_io_sys::CompletionPort;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
use windows_topology_sys::{MachineMemoryTopology, ProcessorId};
use windows_waitable_queues::{Options, PushError, TryRecvError, spsc};

use crate::platform::{self, Residency};
use crate::reader::{Finished, Job, Reader};
use crate::{
    Arrangement, BlockResult, Config, Distribution, checksum_checked, failed, invalid, trial_order,
    verify,
};

#[derive(Debug, Serialize)]
pub struct Capture {
    pub schema: &'static str,
    pub status: &'static str,
    pub build: BuildProvenance,
    pub config: Config,
    pub topology: MachineMemoryTopology,
    pub processors: [ProcessorId; 2],
    pub file_bytes: u64,
    pub blocks: usize,
    pub payload_pool_bytes: usize,
    pub recorded_unix_seconds: u64,
    pub debug_assertions: bool,
    pub evidence_class: &'static str,
    pub caveats: [&'static str; 6],
    pub trials: Vec<Trial>,
}

#[derive(Debug, Serialize)]
pub struct BuildProvenance {
    pub git_revision: &'static str,
    pub worktree_at_build: &'static str,
    pub tracked_diff_fnv64: &'static str,
    pub harness_and_manifests_fnv64: &'static str,
    pub compiler: &'static str,
    pub target: &'static str,
    pub opt_level: &'static str,
    pub rustflags: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Trial {
    pub repetition: usize,
    pub position: usize,
    pub arrangement: Arrangement,
    pub work_wall_ns: u64,
    pub joined_wall_ns: u64,
    pub worker_cpu_ns: u64,
    pub completed_blocks: usize,
    pub bytes_per_second: f64,
    pub checksum_latency: Distribution,
    pub observed_read_latency: Distribution,
    pub compute_time: Distribution,
    pub handoff_latency: Option<Distribution>,
    pub workers: Vec<WorkerReport>,
}

#[derive(Debug, Serialize)]
pub struct WorkerReport {
    pub role: &'static str,
    pub requested_processor: ProcessorId,
    pub processor_at_start: ProcessorId,
    pub processor_at_end: ProcessorId,
    pub cpu_ns: u64,
    pub submitted: usize,
    pub completed: usize,
    pub peak_outstanding_reads: usize,
    pub peak_leased_buffers: usize,
    pub handoff_full_observations: usize,
    pub handoff_high_water: Option<usize>,
    pub return_high_water: Option<usize>,
    pub pages_before: Option<Residency>,
    pub pages_after: Option<Residency>,
}

struct WorkerResult {
    report: WorkerReport,
    results: Vec<BlockResult>,
    finished: Instant,
}

struct Gate {
    ready: mpsc::Sender<()>,
    start: mpsc::Receiver<Instant>,
}

impl Gate {
    fn wait(self) -> io::Result<Instant> {
        self.ready
            .send(())
            .map_err(|_| failed("coordinator left during setup"))?;
        drop(self.ready);
        self.start
            .recv()
            .map_err(|_| failed("coordinator cancelled start"))
    }
}

struct Budget<'a> {
    deadline: Instant,
    cancel: &'a AtomicBool,
}

struct CancelOnFailure<'a> {
    cancel: &'a AtomicBool,
    armed: bool,
}

impl Drop for CancelOnFailure<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Budget<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "peer failed"));
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "trial deadline expired",
            ));
        }
        Ok(())
    }
}

enum Role {
    Direct {
        lane: usize,
        lanes: usize,
    },
    Reader {
        send: spsc::Producer<Job>,
        returns: spsc::Consumer<Finished>,
    },
    Processor {
        receive: spsc::Consumer<Job>,
        returns: spsc::Producer<Finished>,
    },
}

fn roles(arrangement: Arrangement, config: &Config) -> io::Result<Vec<Role>> {
    Ok(match arrangement {
        Arrangement::Direct => vec![Role::Direct { lane: 0, lanes: 1 }],
        Arrangement::Independent => vec![
            Role::Direct { lane: 0, lanes: 2 },
            Role::Direct { lane: 1, lanes: 2 },
        ],
        Arrangement::Pipeline => {
            let (send, receive) =
                spsc::bounded_with(config.queue_capacity, Options::new().tracking_high_water())
                    .map_err(|e| invalid(e.to_string()))?;
            let (return_send, returns) =
                spsc::bounded_with(config.depth, Options::new().tracking_high_water())
                    .map_err(|e| invalid(e.to_string()))?;
            vec![
                Role::Reader { send, returns },
                Role::Processor {
                    receive,
                    returns: return_send,
                },
            ]
        }
    })
}

fn direct_loop(reader: &mut Reader<'_>, config: &Config, budget: &Budget<'_>) -> io::Result<()> {
    while !reader.done() {
        budget.check()?;
        let mut progress = reader.submit_available()?;
        if let Some(job) = reader.poll()? {
            reader.reclaim(job.process(config.checksum_passes, false, || budget.check())?);
            progress = true;
        }
        if !progress {
            thread::yield_now();
        }
    }
    Ok(())
}

fn pipeline_loop(
    reader: &mut Reader<'_>,
    send: &spsc::Producer<Job>,
    returns: &spsc::Consumer<Finished>,
    budget: &Budget<'_>,
) -> io::Result<usize> {
    let mut waiting = None;
    let mut full = 0;
    while !reader.done() {
        budget.check()?;
        let mut progress = false;
        match returns.pop() {
            Ok(done) => {
                reader.reclaim(done);
                progress = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return Err(failed("processor lost before rundown")),
            Err(error) => return Err(failed(format!("return queue: {error}"))),
        }
        progress |= reader.submit_available()?;
        if waiting.is_none() {
            waiting = reader.poll()?;
        }
        if let Some(job) = waiting.take() {
            match send.push(job) {
                Ok(()) => progress = true,
                Err(PushError::Full(job)) => {
                    waiting = Some(job);
                    full += 1;
                }
                Err(PushError::Disconnected(_)) => return Err(failed("processor disconnected")),
                Err(_) => return Err(failed("unsupported handoff queue failure")),
            }
        }
        if !progress {
            thread::yield_now();
        }
    }
    Ok(full)
}

fn processor_loop(
    receive: &spsc::Consumer<Job>,
    returns: &spsc::Producer<Finished>,
    blocks: usize,
    passes: u32,
    budget: &Budget<'_>,
) -> io::Result<()> {
    let mut completed = 0;
    while completed < blocks {
        budget.check()?;
        match receive.pop() {
            Ok(job) => {
                let done = job.process(passes, true, || budget.check())?;
                // Every return owns one of the depth buffers, so a depth-sized return queue
                // has room for this buffer even if the reader has not reclaimed its peers.
                returns.push(done).map_err(|e| match e {
                    PushError::Full(_) => failed("return queue violated payload-credit bound"),
                    PushError::Disconnected(_) => failed("reader disconnected"),
                    _ => failed("unsupported return queue failure"),
                })?;
                completed += 1;
            }
            Err(TryRecvError::Empty) => thread::yield_now(),
            Err(TryRecvError::Disconnected) => {
                return Err(failed("reader stopped before all blocks"));
            }
            Err(error) => return Err(failed(format!("handoff queue: {error}"))),
        }
    }
    Ok(())
}

struct TrialContext<'a> {
    config: &'a Config,
    file_bytes: u64,
    blocks: usize,
    cancel: &'a AtomicBool,
}

fn worker(
    context: &TrialContext<'_>,
    role: Role,
    processor: ProcessorId,
    gate: Gate,
) -> io::Result<WorkerResult> {
    let config = context.config;
    platform::pin(processor)?;
    let mut report = WorkerReport {
        role: "processor",
        requested_processor: processor,
        processor_at_start: processor,
        processor_at_end: processor,
        cpu_ns: 0,
        submitted: 0,
        completed: 0,
        peak_outstanding_reads: 0,
        peak_leased_buffers: 0,
        handoff_full_observations: 0,
        handoff_high_water: None,
        return_high_water: None,
        pages_before: None,
        pages_after: None,
    };
    if let Role::Processor { receive, returns } = role {
        let start = gate.wait()?;
        let budget = Budget {
            deadline: start + Duration::from_millis(config.timeout_ms),
            cancel: context.cancel,
        };
        report.processor_at_start = platform::current_processor();
        let cpu_start = platform::cpu_ns()?;
        processor_loop(
            &receive,
            &returns,
            context.blocks,
            config.checksum_passes,
            &budget,
        )?;
        report.cpu_ns = platform::cpu_ns()? - cpu_start;
        report.processor_at_end = platform::current_processor();
        let finished = Instant::now();
        report.completed = context.blocks;
        report.return_high_water = returns.high_water();
        return Ok(WorkerResult {
            report,
            results: Vec::new(),
            finished,
        });
    }
    let (lane, lanes) = match &role {
        Role::Direct { lane, lanes } => (*lane, *lanes),
        _ => (0, 1),
    };
    let port = CompletionPort::new(1)?;
    let mut reader = Reader::new(
        &port,
        config,
        context.file_bytes,
        context.blocks,
        lane,
        lanes,
    )?;
    report.pages_before = Some(platform::residency(&reader.free)?);
    let start = gate.wait()?;
    let budget = Budget {
        deadline: start + Duration::from_millis(config.timeout_ms),
        cancel: context.cancel,
    };
    report.processor_at_start = platform::current_processor();
    let cpu_start = platform::cpu_ns()?;
    match role {
        Role::Direct { .. } => {
            report.role = "direct";
            direct_loop(&mut reader, config, &budget)?;
        }
        Role::Reader { send, returns } => {
            report.role = "reader";
            report.handoff_full_observations =
                pipeline_loop(&mut reader, &send, &returns, &budget)?;
            report.handoff_high_water = send.high_water();
        }
        Role::Processor { .. } => unreachable!(),
    }
    report.cpu_ns = platform::cpu_ns()? - cpu_start;
    report.processor_at_end = platform::current_processor();
    let finished = Instant::now();
    report.submitted = reader.submitted;
    report.completed = reader.results.len();
    report.peak_outstanding_reads = reader.peak_io;
    report.peak_leased_buffers = reader.peak_leased;
    if port.outstanding() != 0 || reader.free.len() != config.depth / lanes {
        return Err(failed(
            "trial completed without draining all I/O and buffer credits",
        ));
    }
    report.pages_after = Some(platform::residency(&reader.free)?);
    Ok(WorkerResult {
        report,
        results: reader.results,
        finished,
    })
}

fn run_trial(
    config: &Config,
    file_bytes: u64,
    expected: &[u64],
    pair: [ProcessorId; 2],
    arrangement: Arrangement,
    repetition: usize,
    position: usize,
) -> io::Result<Trial> {
    let cancel = AtomicBool::new(false);
    let context = TrialContext {
        config,
        file_bytes,
        blocks: expected.len(),
        cancel: &cancel,
    };
    let (start, joined, workers) = thread::scope(|scope| -> io::Result<_> {
        let (ready_send, ready_receive) = mpsc::channel();
        let mut starts = Vec::new();
        let mut handles = Vec::new();
        for (index, role) in roles(arrangement, config)?.into_iter().enumerate() {
            let (send, receive) = mpsc::channel();
            starts.push(send);
            let gate = Gate {
                ready: ready_send.clone(),
                start: receive,
            };
            let context = &context;
            handles.push(
                thread::Builder::new()
                    .name(format!("read-checksum-{index}"))
                    .spawn_scoped(scope, move || {
                        let mut failure = CancelOnFailure {
                            cancel: context.cancel,
                            armed: true,
                        };
                        let outcome = worker(context, role, pair[index], gate);
                        failure.armed = outcome.is_err();
                        outcome
                    })?,
            );
        }
        drop(ready_send);
        let setup_deadline = Instant::now() + Duration::from_millis(config.timeout_ms);
        let mut start_error = None;
        for _ in &starts {
            let remaining = setup_deadline.saturating_duration_since(Instant::now());
            if let Err(error) = ready_receive.recv_timeout(remaining) {
                start_error = Some(failed(format!("worker setup did not complete: {error}")));
                break;
            }
        }
        let start = Instant::now();
        if start_error.is_none() {
            for send in &starts {
                if send.send(start).is_err() {
                    start_error = Some(failed("worker exited before start"));
                    break;
                }
            }
        }
        drop(starts);
        if start_error.is_some() {
            cancel.store(true, Ordering::Relaxed);
        }
        let mut workers = Vec::new();
        let mut errors = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(result)) => workers.push(result),
                Ok(Err(error)) => errors.push(error.to_string()),
                Err(_) => {
                    cancel.store(true, Ordering::Relaxed);
                    errors.push("worker panicked".to_owned());
                }
            }
        }
        if let Some(error) = start_error {
            errors.push(error.to_string());
        }
        if !errors.is_empty() {
            return Err(failed(errors.join("; ")));
        }
        Ok((start, Instant::now(), workers))
    })?;
    let mut work_wall_ns = 0;
    let mut reports = Vec::new();
    let mut results = Vec::with_capacity(expected.len());
    for worker in workers {
        work_wall_ns = work_wall_ns.max(worker.finished.duration_since(start).as_nanos() as u64);
        if worker.report.processor_at_start != worker.report.requested_processor
            || worker.report.processor_at_end != worker.report.requested_processor
        {
            return Err(failed("worker did not retain its processor binding"));
        }
        results.extend(worker.results);
        reports.push(worker.report);
    }
    verify(&mut results, expected)?;
    Ok(Trial {
        repetition,
        position,
        arrangement,
        work_wall_ns,
        joined_wall_ns: joined.duration_since(start).as_nanos() as u64,
        worker_cpu_ns: reports.iter().map(|worker| worker.cpu_ns).sum(),
        completed_blocks: results.len(),
        bytes_per_second: file_bytes as f64 * 1_000_000_000.0 / work_wall_ns as f64,
        checksum_latency: Distribution::from_values(results.iter().map(|r| r.latency_ns).collect()),
        observed_read_latency: Distribution::from_values(
            results.iter().map(|r| r.read_ns).collect(),
        ),
        compute_time: Distribution::from_values(results.iter().map(|r| r.compute_ns).collect()),
        handoff_latency: (arrangement == Arrangement::Pipeline)
            .then(|| Distribution::from_values(results.iter().map(|r| r.queue_ns).collect())),
        workers: reports,
    })
}

pub fn run(config: Config) -> io::Result<Capture> {
    let mut fixture = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&config.file)?;
    let metadata = fixture.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("fixture must be a regular file"));
    }
    let file_bytes = metadata.len();
    let blocks = config.validate(file_bytes)?;
    let topology = MachineMemoryTopology::discover()?;
    let pair = platform::select_processors(&topology, config.processors)?;
    let mut expected = Vec::with_capacity(blocks);
    let mut buffer = vec![0; config.block_bytes];
    let cancel = AtomicBool::new(false);
    let budget = Budget {
        deadline: Instant::now() + Duration::from_millis(config.timeout_ms),
        cancel: &cancel,
    };
    for id in 0..blocks {
        let bytes = (file_bytes - id as u64 * config.block_bytes as u64)
            .min(config.block_bytes as u64) as usize;
        fixture.read_exact(&mut buffer[..bytes])?;
        expected.push(checksum_checked(
            &buffer[..bytes],
            config.checksum_passes,
            || budget.check(),
        )?);
    }

    drop(buffer);
    for arrangement in trial_order(0) {
        run_trial(&config, file_bytes, &expected, pair, arrangement, 0, 0)?;
    }
    let mut trials = Vec::with_capacity(config.repetitions * 3);
    for repetition in 0..config.repetitions {
        for (position, arrangement) in trial_order(repetition).into_iter().enumerate() {
            trials.push(run_trial(
                &config,
                file_bytes,
                &expected,
                pair,
                arrangement,
                repetition,
                position,
            )?);
        }
    }
    // The handle denying writes/deletion stays alive through reference validation and all trials.
    drop(fixture);
    Ok(Capture {
        schema: "read-checksum-v1",
        status: "success",
        build: BuildProvenance {
            git_revision: env!("RC_GIT_REVISION"),
            worktree_at_build: env!("RC_WORKTREE"),
            tracked_diff_fnv64: env!("RC_TRACKED_DIFF_FNV64"),
            harness_and_manifests_fnv64: env!("RC_HARNESS_AND_MANIFESTS_FNV64"),
            compiler: env!("RC_COMPILER"),
            target: env!("RC_TARGET"),
            opt_level: env!("RC_OPT_LEVEL"),
            rustflags: env!("RC_RUSTFLAGS"),
        },
        payload_pool_bytes: config.block_bytes * config.depth,
        config,
        topology,
        processors: pair,
        file_bytes,
        blocks,
        recorded_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs(),
        debug_assertions: cfg!(debug_assertions),
        evidence_class: "buffered_file_closed_loop_after_reference_read",
        caveats: [
            "Reference read warms cache; cache residency is not guaranteed. This is not raw device bandwidth.",
            "Latency begins at admission, not offered arrival; no saturation responsiveness claim.",
            "Payload pool is bounded equally; queue, token, result, thread and allocator metadata are extra.",
            "Worker CPU excludes setup and residency sampling; work wall ends when every worker finishes its measured loop.",
            "Joined wall additionally includes post-run residency sampling and thread teardown; CPU timer has OS accounting granularity.",
            "Only payload-buffer pages are sampled, including partial heap boundary pages; heap/queue/stack placement is not controlled.",
        ],
        trials,
    })
}

#[cfg(test)]
mod tests;
