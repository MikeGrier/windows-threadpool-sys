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
    Arrangement, BlockResult, Comparison, Config, Distribution, InputKind, checksum_checked,
    comparison_order, failed, fill_fixture, invalid, verify,
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
    pub caveats: [&'static str; 7],
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
    pub reversed: bool,
    pub resources: Resources,
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
pub struct Resources {
    pub worker_threads: usize,
    pub participating_processors: usize,
    pub checksum_workers: usize,
    pub unequal_cpu_reference: bool,
    pub max_pending_reads: usize,
    pub payload_pool_bytes: usize,
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
    pub checksummed_blocks: usize,
    pub buffer_capacity: usize,
    pub numa_backed_buffers: usize,
    pub read_capacity: usize,
    pub buffer_pressure_observations: usize,
    pub batches: BatchStats,
    pub return_batches: BatchStats,
    pub peak_outstanding_reads: usize,
    pub peak_leased_buffers: usize,
    pub handoff_full_observations: usize,
    pub handoff_high_water: Option<usize>,
    pub return_high_water: Option<usize>,
    pub pages_before: Option<Residency>,
    pub pages_after: Option<Residency>,
}

#[derive(Debug, Default, Serialize)]
pub struct BatchStats {
    pub count: usize,
    pub jobs: usize,
    pub max_jobs: usize,
    pub partial: usize,
}

impl BatchStats {
    fn record(&mut self, jobs: usize, limit: usize) {
        if jobs != 0 {
            self.count += 1;
            self.jobs += jobs;
            self.max_jobs = self.max_jobs.max(jobs);
            self.partial += usize::from(jobs < limit);
        }
    }
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
                spsc::bounded_with(config.buffers(), Options::new().tracking_high_water())
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

fn direct_loop(
    reader: &mut Reader<'_>,
    config: &Config,
    budget: &Budget<'_>,
    batches: &mut BatchStats,
) -> io::Result<()> {
    while !reader.done() {
        budget.check()?;
        let mut progress = reader.submit_available(|| budget.check())?;
        let mut processed = 0;
        for _ in 0..config.batch_size {
            budget.check()?;
            let Some(job) = reader.poll()? else { break };
            reader.reclaim(job.process(config.checksum_passes, false, || budget.check())?);
            processed += 1;
            progress = true;
        }
        batches.record(processed, config.batch_size);
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
    config: &Config,
    budget: &Budget<'_>,
    report: &mut WorkerReport,
) -> io::Result<usize> {
    let mut waiting = None;
    let mut full = 0;
    while !reader.done() {
        budget.check()?;
        let mut progress = false;
        let mut returned = 0;
        for _ in 0..config.batch_size {
            budget.check()?;
            match returns.pop() {
                Ok(done) => {
                    reader.reclaim(done);
                    returned += 1;
                    progress = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(failed("processor lost before rundown"));
                }
                Err(error) => return Err(failed(format!("return queue: {error}"))),
            }
            if reader.done() {
                break;
            }
        }
        report.return_batches.record(returned, config.batch_size);
        progress |= reader.submit_available(|| budget.check())?;
        let mut forwarded = 0;
        for _ in 0..config.batch_size {
            budget.check()?;
            if waiting.is_none() {
                waiting = reader.poll()?;
            }
            let Some(job) = waiting.take() else { break };
            match send.push(job) {
                Ok(()) => {
                    progress = true;
                    forwarded += 1;
                }
                Err(PushError::Full(job)) => {
                    waiting = Some(job);
                    full += 1;
                    break;
                }
                Err(PushError::Disconnected(_)) => return Err(failed("processor disconnected")),
                Err(_) => return Err(failed("unsupported handoff queue failure")),
            }
        }
        report.batches.record(forwarded, config.batch_size);
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
    config: &Config,
    budget: &Budget<'_>,
    batches: &mut BatchStats,
) -> io::Result<()> {
    let processing_quantum = config.batch_size;
    let mut completed = 0;
    while completed < blocks {
        budget.check()?;
        let mut processed = 0;
        for _ in 0..processing_quantum {
            if completed == blocks {
                break;
            }
            budget.check()?;
            match receive.pop() {
                Ok(job) => {
                    let done = job.process(config.checksum_passes, true, || budget.check())?;
                    // Every return owns one of the pool buffers, so a pool-sized return queue
                    // has room for this buffer even if the reader has not reclaimed its peers.
                    returns.push(done).map_err(|e| match e {
                        PushError::Full(_) => failed("return queue violated payload-credit bound"),
                        PushError::Disconnected(_) => failed("reader disconnected"),
                        _ => failed("unsupported return queue failure"),
                    })?;
                    completed += 1;
                    processed += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(failed("reader stopped before all blocks"));
                }
                Err(error) => return Err(failed(format!("handoff queue: {error}"))),
            }
        }
        batches.record(processed, processing_quantum);
        if processed == 0 {
            thread::yield_now();
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
        checksummed_blocks: 0,
        buffer_capacity: 0,
        numa_backed_buffers: 0,
        read_capacity: 0,
        buffer_pressure_observations: 0,
        batches: BatchStats::default(),
        return_batches: BatchStats::default(),
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
            config,
            &budget,
            &mut report.batches,
        )?;
        report.cpu_ns = platform::cpu_ns()? - cpu_start;
        report.processor_at_end = platform::current_processor();
        let finished = Instant::now();
        report.completed = context.blocks;
        report.checksummed_blocks = context.blocks;
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
    report.buffer_capacity = reader.free.len();
    report.numa_backed_buffers = reader.free.iter().filter(|buffer| buffer.is_numa()).count();
    report.read_capacity = reader.read_limit;
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
            direct_loop(&mut reader, config, &budget, &mut report.batches)?;
            report.checksummed_blocks = reader.results.len();
        }
        Role::Reader { send, returns } => {
            report.role = "reader";
            report.handoff_full_observations =
                pipeline_loop(&mut reader, &send, &returns, config, &budget, &mut report)?;
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
    report.buffer_pressure_observations = reader.buffer_pressure_observations;
    if port.outstanding() != 0 || reader.free.len() != config.buffers() / lanes {
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
    comparison: Comparison,
    repetition: usize,
    position: usize,
) -> io::Result<Trial> {
    let arrangement = comparison.arrangement;
    let pair = comparison.processors(pair);
    let roles = roles(arrangement, config)?;
    let worker_threads = roles.len();
    let checksum_workers = roles
        .iter()
        .filter(|role| !matches!(role, Role::Reader { .. }))
        .count();
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
        for (index, role) in roles.into_iter().enumerate() {
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
    let max_pending_reads = reports.iter().map(|report| report.read_capacity).sum();
    let payload_pool_bytes = reports
        .iter()
        .map(|report| report.buffer_capacity * config.block_bytes)
        .sum();
    Ok(Trial {
        repetition,
        position,
        arrangement,
        reversed: comparison.reversed,
        resources: Resources {
            worker_threads,
            participating_processors: worker_threads,
            checksum_workers,
            unequal_cpu_reference: worker_threads == 1,
            max_pending_reads,
            payload_pool_bytes,
        },
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
    let mut fixture = if config.input == InputKind::BufferedFile {
        Some(
            OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&config.file)?,
        )
    } else {
        None
    };
    let file_bytes = if let Some(fixture) = &fixture {
        let metadata = fixture.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("fixture must be a regular file"));
        }
        metadata.len()
    } else {
        config
            .generated_bytes
            .ok_or_else(|| invalid("generated_bytes is required"))?
    };
    let blocks = config.validate(file_bytes)?;
    let topology = MachineMemoryTopology::discover()?;
    if config
        .payload_node
        .is_some_and(|node| !platform::memory_nodes(&topology).contains(&node))
    {
        return Err(invalid(
            "payload_node is not an observed Windows memory node",
        ));
    }
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
        if let Some(fixture) = &mut fixture {
            fixture.read_exact(&mut buffer[..bytes])?;
        } else {
            fill_fixture(&mut buffer[..bytes], id as u64 * config.block_bytes as u64);
        }
        expected.push(checksum_checked(
            &buffer[..bytes],
            config.checksum_passes,
            || budget.check(),
        )?);
    }

    drop(buffer);
    for comparison in comparison_order(0) {
        run_trial(&config, file_bytes, &expected, pair, comparison, 0, 0)?;
    }
    let mut trials = Vec::with_capacity(config.repetitions * comparison_order(0).len());
    for repetition in 0..config.repetitions {
        for (position, comparison) in comparison_order(repetition).into_iter().enumerate() {
            trials.push(run_trial(
                &config, file_bytes, &expected, pair, comparison, repetition, position,
            )?);
        }
    }
    // The handle denying writes/deletion stays alive through reference validation and all trials.
    drop(fixture);
    let evidence_class = match config.input {
        InputKind::BufferedFile => "buffered_file_closed_loop_after_reference_read",
        InputKind::Generated => "generated_buffers_closed_loop_including_producer_fill",
    };
    Ok(Capture {
        schema: "read-checksum-v4",
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
        payload_pool_bytes: config.block_bytes * config.buffers(),
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
        evidence_class,
        caveats: [
            "Buffered-file reference reads warm cache without guaranteeing residency; generated input fills payloads on the producer inside timing. Neither is raw device bandwidth.",
            "Latency begins at admission, not offered arrival; no saturation responsiveness claim.",
            "Payload pool is bounded equally; queue, token, result, thread and allocator metadata are extra.",
            "Worker CPU excludes setup and residency sampling; work wall ends when every worker finishes its measured loop.",
            "Joined wall additionally includes post-run residency sampling and thread teardown; CPU timer has OS accounting granularity.",
            "Only payload-buffer pages are sampled, including partial heap boundary pages; heap/queue/stack placement is not controlled.",
            "NUMA allocation is a preference, not proof of placement. Residency labels are incomplete when node_ids_truncated is true; unresident pages are unknown.",
        ],
        trials,
    })
}

#[cfg(test)]
mod tests;
