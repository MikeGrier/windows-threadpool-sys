// Copyright (c) 2026 Mike Grier
//! Generated operation sequences, checked against the ring contract (M17.3).
//!
//! Every other test in this crate states a scenario by hand. That is how
//! [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47)
//! survived: it was one point in a space nobody was sampling. This file models
//! the space as *data* -- operation kind, file target kind, buffer kind,
//! claim-or-drop, when to drain, and where the completion event is attached --
//! and generates sequences over it.
//!
//! **This is an input generator for detectors that already exist**, not a new
//! detector. Each sequence runs under [`windows_guard_alloc::GuardAlloc`] so a
//! use-after-free faults instead of silently reading stale bytes (M15), and is
//! checked against [`RingContract`] so a lost, duplicated, or unclaimed
//! completion is reported as a contract violation rather than inferred (M16).
//! Without those, a generator would only be checking that nothing crashed.
//!
//! **Calibrated, and it failed the first time.** M17.4 reverted D-20's setup
//! signal -- #47 exactly as it shipped -- and this file reported green. It
//! attached the event and sampled the right states, but drained by polling
//! `try_pop`, which recovers every completion whether or not the ring ever
//! signalled. Sampling the right state is not the same as being sensitive to
//! the defect that lives in it. Hence [`wait_then_drain`]: once an event is
//! attached, a sequence waits for the wakeup it is owed *before* draining.
//! **Do not "simplify" that back into a poll** -- it is load-bearing, and
//! removing it silently restores a generator that cannot see #47. With it, the
//! defect is caught in 10 of 10 runs on fresh seeds. See `D-42`.
//!
//! **Seeding, per [DESIGN-NOTES.md](DESIGN-NOTES.md) `D-41`.** One number
//! replays a whole run, it is announced with the command to replay it, and it
//! is pinnable from the environment. There are deliberately **two** seeds --
//! this file's, which picks the sequences, and the guard allocator's, which
//! picks the poison pattern -- and they are separate knobs. Conflating them in
//! the announcement would produce a replay that reproduces one and not the
//! other, so both are printed and both are pinned independently.
//!
//! **Why no `proptest`, and no shrinking.** D-41's terms are that *one number*
//! replays a whole run and is pinnable from the environment; `proptest`'s
//! reproducibility model is a persisted regression file plus per-case seeds,
//! which is a different shape, and its headline feature -- shrinking -- earns
//! its keep on long sequences. These are capped at [`MAX_STEPS`] steps and every
//! step is printed, so a failure already arrives as a trace short enough to read
//! and hand-copy into the named regression test M17.5 calls for. If that cap is
//! ever raised substantially, shrinking stops being redundant and this decision
//! should be revisited; a `SplitMix64` over one `u64` is what serves the stated
//! terms at this size.

#![cfg(windows)]

use std::collections::HashMap;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::PathBuf;

use windows_ioring_sys::contract::RingContract;
use windows_ioring_sys::{
    Batch, Completion, FlushCoverage, FlushMode, IoRing, PushOptions, RegisteredBuffers,
    RegisteredFile, RegisteredSpan, RegisteredUse, SharedFile, Token, WriteCaching,
};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

/// A use-after-free in a generated sequence must fault rather than read stale
/// bytes, or the generator is only testing that nothing happened to crash.
#[global_allocator]
static ALLOC: windows_guard_alloc::GuardAlloc = windows_guard_alloc::GuardAlloc::new();

/// Environment override for this file's generator seed (decimal, or `0x` hex).
/// The guard allocator has its own, separate variable.
const SEED_VAR: &str = "WINDOWS_IORING_SEQ_SEED";

/// Sequences per run, and the longest sequence generated.
///
/// The count is set by the coverage assertion at the end of this file, not by
/// taste: that assertion demands every shape appear, and the rarest -- a flush
/// against a registered file, at 10% x 1/3 of operations -- would otherwise be
/// missed often enough to flake. At roughly five operations per sequence this
/// yields ~650, where the chance of missing a 3.3% shape is about 3e-10 rather
/// than the 2.5e-4 that 48 sequences gave. The whole file still runs in well
/// under a second.
const SEQUENCES: usize = 128;
const MAX_STEPS: usize = 10;

const BUF_LEN: usize = 512;

/// Registered buffers available to a sequence. More than one so the generator
/// can keep several registered operations outstanding at the same time.
const REGISTERED_BUFFERS: u32 = 4;

/// Reads address `[0, WRITE_BASE)` and writes address `[WRITE_BASE, ..)`, so a
/// generated write can never change what a generated read is expected to see.
const WRITE_BASE: u64 = 64 * BUF_LEN as u64;
const FILE_LEN: usize = 128 * BUF_LEN;

/// Bounded so a lost completion fails instead of hanging.
const MAX_DRAIN_ROUNDS: usize = 4096;

/// How long to wait for a wakeup that the ring owes an attached waiter.
///
/// Generous, because a wait that is *expected* to succeed must not flake on a
/// loaded machine. It is only ever paid in full by a run that is about to fail,
/// and a run that pays it is reporting a lost wakeup -- which is #47.
const WAIT_MS: u32 = 5_000;

// --- the model ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum OpKind {
    Read,
    Write,
    Flush,
}

/// How the operation names its file. All three are real entry points with
/// different addressing, and only the raw one is `unsafe`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum TargetKind {
    Raw,
    Shared,
    Registered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum BufferKind {
    Owned,
    Registered,
}

/// What the generated sequences actually exercised.
///
/// D-41's corollary applies to a generator as much as to a timing-dependent
/// test: a run that reports green proves nothing unless it also shows it
/// reached the states it claims to sample. Without this, a change that made
/// every registered-buffer operation quietly downgrade to an owned one would
/// still pass, having tested a third of the space it advertises.
#[derive(Default)]
struct Coverage {
    shapes: std::collections::HashSet<(OpKind, TargetKind, BufferKind)>,
    deliberate_drops: usize,
    attaches: usize,
    attaches_with_work_outstanding: usize,
    mid_sequence_drains: usize,
    buffer_downgrades: usize,
    operations: usize,
}

#[derive(Clone, Copy, Debug)]
struct GenOp {
    kind: OpKind,
    target: TargetKind,
    buffer: BufferKind,
    /// Claim the token against its completion, or drop it deliberately. A
    /// deliberate drop is a legitimate choice -- it is what keeps a buffer
    /// alive when the caller cannot prove the kernel is done -- so the
    /// contract is told about it rather than reporting a leak.
    claim: bool,
    drain_preceding: bool,
    slot: u64,
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Op(GenOp),
    /// Drain the completion queue to empty right here, rather than letting
    /// completions accumulate.
    DrainNow,
    /// Attach the completion event at this point in the sequence, so the
    /// handover happens against whatever ring state the steps so far produced.
    /// This is #47's axis, generated rather than enumerated.
    Attach,
}

#[derive(Clone, Debug)]
struct Plan {
    steps: Vec<Step>,
}

// A plan states intent; what a failure needs is what actually *ran*, since the
// executor legitimately adjusts a step when the ring state rules it out (all
// registered buffers busy, for one). The trace is recorded during execution and
// is what gets printed, so a reported sequence can be hand-copied into the named
// regression test M17.5 calls for without re-deriving anything.

// --- seeded generation -------------------------------------------------------

/// SplitMix64. Chosen because one `u64` of state means the seed *is* the run,
/// which is what D-41 requires; the statistical quality needed here is low.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// Resolve the generator seed: pinned from the environment when set, otherwise
/// derived from the clock so successive runs explore different sequences.
fn resolve_seed() -> u64 {
    match std::env::var(SEED_VAR) {
        Ok(text) => {
            let trimmed = text.trim();
            let parsed = trimmed
                .strip_prefix("0x")
                .or_else(|| trimmed.strip_prefix("0X"))
                .map_or_else(
                    || trimmed.parse::<u64>().ok(),
                    |hex| u64::from_str_radix(hex, 16).ok(),
                );
            parsed.unwrap_or_else(|| {
                panic!("{SEED_VAR} is set to {text:?}, which is neither decimal nor 0x hex")
            })
        }
        Err(_) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x243F_6A88_85A3_08D3, |elapsed| elapsed.as_nanos() as u64),
    }
}

fn generate(rng: &mut Rng) -> Plan {
    let step_count = 3 + rng.below(MAX_STEPS as u64 - 2) as usize;
    let mut steps = Vec::with_capacity(step_count);

    // At most one attach per sequence, placed anywhere including before any
    // operation (the fresh-ring case) and after several (the backlog case).
    let attach_at = if rng.chance(60) {
        Some(rng.below(step_count as u64) as usize)
    } else {
        None
    };

    for index in 0..step_count {
        if attach_at == Some(index) {
            steps.push(Step::Attach);
        }
        if rng.chance(20) {
            steps.push(Step::DrainNow);
            continue;
        }
        let kind = match rng.below(10) {
            0..=5 => OpKind::Read,
            6..=8 => OpKind::Write,
            _ => OpKind::Flush,
        };
        let target = match rng.below(3) {
            0 => TargetKind::Raw,
            1 => TargetKind::Shared,
            _ => TargetKind::Registered,
        };
        // A flush carries no buffer, so its buffer kind is not a free axis.
        let buffer = if kind == OpKind::Flush || rng.chance(45) {
            BufferKind::Owned
        } else {
            BufferKind::Registered
        };
        steps.push(Step::Op(GenOp {
            kind,
            target,
            buffer,
            claim: rng.chance(85),
            drain_preceding: rng.chance(15),
            slot: rng.below(16),
        }));
    }
    Plan { steps }
}

// --- what a sequence holds while it runs -------------------------------------

/// One outstanding operation's token. The token type differs per path -- the
/// raw entry points hand back a bare buffer, the targeted ones a buffer plus a
/// guard keeping the file alive -- so this enum is what lets one sequence mix
/// them and still claim each against its own completion.
enum Held {
    RawOwned(Token<Vec<u8>>),
    RawRegistered(Token<RegisteredUse>),
    SharedOwned(Token<(Vec<u8>, SharedFile)>),
    SharedRegistered(Token<(RegisteredUse, SharedFile)>),
    SharedFlush(Token<SharedFile>),
    RegdOwned(Token<(Vec<u8>, RegisteredFile)>),
    RegdRegistered(Token<(RegisteredUse, RegisteredFile)>),
    RegdFlush(Token<RegisteredFile>),
}

impl Held {
    fn claim(self, completion: &Completion) -> Result<(), ()> {
        match self {
            Held::RawOwned(token) => token.claim_if(completion).map(|_| ()).map_err(|_| ()),
            Held::RawRegistered(token) => token.claim_if(completion).map(|_| ()).map_err(|_| ()),
            Held::SharedOwned(token) => token.claim_if(completion).map(|_| ()).map_err(|_| ()),
            Held::SharedRegistered(token) => token.claim_if(completion).map(|_| ()).map_err(|_| ()),
            Held::SharedFlush(token) => token.claim_if(completion).map(|_| ()).map_err(|_| ()),
            Held::RegdOwned(token) => token.claim_if(completion).map(|_| ()).map_err(|_| ()),
            Held::RegdRegistered(token) => token.claim_if(completion).map(|_| ()).map_err(|_| ()),
            Held::RegdFlush(token) => token.claim_if(completion).map(|_| ()).map_err(|_| ()),
        }
    }
}

struct Run {
    contract: RingContract,
    held: HashMap<usize, Held>,
    /// Operations whose token was dropped on purpose. Their completions still
    /// arrive and must still be popped; they simply have nothing to claim.
    dropped: HashMap<usize, ()>,
    /// Which registered buffer index each outstanding operation is using, so
    /// the generator never points two live operations at the same buffer --
    /// that would be a data race this crate cannot be blamed for.
    buffer_in_use: HashMap<usize, u32>,
    /// What actually ran, step by step, for the failure report.
    trace: Vec<String>,
}

// --- executing a plan --------------------------------------------------------

/// Tagged per test, not just per process: libtest runs the tests in this file
/// concurrently as threads, so one shared path would have them writing and
/// opening the same file at the same time.
fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-generated-{tag}-{}.tmp",
        std::process::id()
    ))
}

fn fixture(tag: &str) -> (File, PathBuf) {
    let path = temp_file(tag);
    let mut content = vec![0_u8; FILE_LEN];
    for (index, chunk) in content.chunks_mut(BUF_LEN).enumerate() {
        chunk.fill(index as u8);
    }
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open fixture read/write");
    (file, path)
}

fn duplicate_handle(file: &File) -> OwnedHandle {
    file.try_clone()
        .expect("duplicate the fixture handle")
        .into()
}

/// Drain to empty once, claiming what can be claimed, and report how many
/// completions this pass observed.
fn drain_to_empty(ring: &mut IoRing, run: &mut Run) -> usize {
    let mut popped = 0;
    while let Some(completion) = ring.try_pop().expect("pop completion") {
        let user_data = completion.user_data();
        run.contract.observe_completion(user_data);
        // The result itself is not asserted: a generated write to an odd offset
        // or a flush on a busy file may legitimately fail. What must hold is
        // that the completion arrives exactly once and its token is settled.
        let _ = completion.result();
        run.buffer_in_use.remove(&user_data);
        if let Some(held) = run.held.remove(&user_data) {
            held.claim(&completion)
                .expect("a token must claim its own completion");
            run.contract.observe_claim(user_data);
        } else if run.dropped.remove(&user_data).is_some() {
            run.contract.observe_deliberate_leak(user_data);
        }
        popped += 1;
    }
    popped
}

fn run_plan(
    plan: &Plan,
    file: &File,
    shared: &SharedFile,
    coverage: &mut Coverage,
) -> Result<(), String> {
    let handle = file.as_raw_handle();
    let mut ring = IoRing::new(64, 64).expect("create ring");
    let mut run = Run {
        contract: RingContract::new(),
        held: HashMap::new(),
        dropped: HashMap::new(),
        buffer_in_use: HashMap::new(),
        trace: Vec::new(),
    };

    // Registrations are set up once, synchronously, before the plan runs: they
    // are a precondition of the registered variants rather than part of the
    // space being sampled here.
    let registered_file = {
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `file` outlives this ring -- the caller owns it for the whole
        // test and every operation is drained before this function returns.
        let pending = unsafe { batch.register_files(&[handle]) }.expect("queue file registration");
        batch.submit_and_wait(1, 5_000).expect("submit");
        let completion = ring
            .try_pop()
            .expect("pop")
            .expect("a registration completion is ready");
        pending
            .claim_if(&completion)
            .expect("registration token claims its own completion")
            .expect("file registration succeeded")
    };
    let registered_file = registered_file.get(0).expect("index 0 exists");

    let registered_buffers: RegisteredBuffers<Vec<u8>> = {
        let mut batch = Batch::new(&mut ring);
        let pending = batch
            .register_buffers(
                (0..REGISTERED_BUFFERS)
                    .map(|_| vec![0_u8; BUF_LEN])
                    .collect(),
            )
            .expect("queue buffer registration");
        batch.submit_and_wait(1, 5_000).expect("submit");
        let completion = ring
            .try_pop()
            .expect("pop")
            .expect("a registration completion is ready");
        pending
            .claim_if(&completion)
            .expect("registration token claims its own completion")
            .expect("buffer registration succeeded")
    };

    let mut event: Option<OwnedHandle> = None;

    for step in &plan.steps {
        match step {
            Step::DrainNow => {
                coverage.mid_sequence_drains += 1;
                match wait_then_drain(&mut ring, &mut run, event.as_ref(), WAIT_MS) {
                    Ok(popped) => run.trace.push(format!("drain to empty ({popped} popped)")),
                    Err(lost) => {
                        run.trace.push("drain to empty".to_owned());
                        let report = format!("{lost}\ntrace:\n{}", render(&run.trace));
                        forfeit(registered_buffers);
                        return Err(report);
                    }
                }
            }
            Step::Attach => {
                // Repeat attaches hand back a duplicate of the same event
                // (D-20), so this is safe to reach more than once.
                coverage.attaches += 1;
                if ring.outstanding() > 0 {
                    coverage.attaches_with_work_outstanding += 1;
                }
                run.trace.push(format!(
                    "attach completion event (queue had {} outstanding)",
                    ring.outstanding()
                ));
                event = Some(ring.completion_event().expect("attach completion event"));
            }
            Step::Op(op) => {
                submit_one(
                    &mut ring,
                    &mut run,
                    op,
                    handle,
                    shared,
                    registered_file,
                    &registered_buffers,
                    coverage,
                );
            }
        }
    }

    // Settle: everything submitted must complete and be accounted for.
    let mut rounds = 0;
    while ring.outstanding() > 0 {
        rounds += 1;
        if rounds > MAX_DRAIN_ROUNDS {
            let report = format!(
                "{} operations never completed after {rounds} drain rounds\ntrace:\n{}",
                ring.outstanding(),
                render(&run.trace)
            );
            forfeit(registered_buffers);
            return Err(report);
        }
        if let Err(lost) = wait_then_drain(&mut ring, &mut run, event.as_ref(), WAIT_MS) {
            let report = format!("{lost}\ntrace:\n{}", render(&run.trace));
            forfeit(registered_buffers);
            return Err(report);
        }
    }
    drain_to_empty(&mut ring, &mut run);

    for index in 0..REGISTERED_BUFFERS {
        if let Some(outstanding) = registered_buffers.outstanding(index) {
            run.contract.observe_buffer(index, outstanding);
        }
    }

    let violations = run.contract.check_quiescent();
    drop(event);
    if violations.is_empty() {
        Ok(())
    } else {
        let report = format!("{violations:?}\ntrace:\n{}", render(&run.trace));
        forfeit(registered_buffers);
        Err(report)
    }
}

/// Abandon a registration on a failure path.
///
/// [`RegisteredBuffers`]'s own drop guard `debug_assert`s when a buffer is
/// still outstanding (M5.3), which on a failing sequence fires *while this
/// function returns* and replaces the contract's report -- naming neither the
/// sequence, the step, nor the seed. Leaking instead is the same choice the
/// crate itself makes in release builds ("leak is safe, use-after-free is
/// not"), and it keeps the diagnostic that says what actually went wrong.
/// Only ever reached on a path that is about to fail the test.
fn forfeit(registration: RegisteredBuffers<Vec<u8>>) {
    std::mem::forget(registration);
}

/// Wait up to `timeout_ms` for `event`, consuming the signal when it fires.
fn signalled_within(event: &OwnedHandle, timeout_ms: u32) -> bool {
    // SAFETY: `event` is a live event handle this sequence owns.
    let result = unsafe { WaitForSingleObject(event.as_raw_handle(), timeout_ms) };
    if result == WAIT_OBJECT_0 {
        true
    } else if result == WAIT_TIMEOUT {
        false
    } else {
        panic!("unexpected WaitForSingleObject result 0x{result:08X}");
    }
}

/// Collect completions the way a consumer actually would.
///
/// This is the difference between a generator that can find #47 and one that
/// cannot. Polling `try_pop` unconditionally recovers every completion whether
/// or not the ring ever signalled, so a lost wakeup is invisible to it -- which
/// is exactly what M17.4's calibration caught. Once an event is attached, the
/// sequence therefore **waits for the wakeup it is owed** before draining, and a
/// wait that times out with work still outstanding is reported rather than
/// papered over by another poll.
///
/// Waiting happens before draining, not after, because a backlog queued *before*
/// the attach is only reachable through the deliberate setup signal (D-20).
/// Draining first would consume that backlog by polling and hide its absence.
fn wait_then_drain(
    ring: &mut IoRing,
    run: &mut Run,
    event: Option<&OwnedHandle>,
    timeout_ms: u32,
) -> Result<usize, String> {
    if ring.outstanding() == 0 {
        return Ok(drain_to_empty(ring, run));
    }
    if let Some(event) = event
        && !signalled_within(event, timeout_ms)
    {
        return Err(format!(
            "waited {timeout_ms} ms on an attached completion event with {} operations \
             outstanding and was never woken -- a wakeup was lost",
            ring.outstanding()
        ));
    }
    Ok(drain_to_empty(ring, run))
}

fn render(trace: &[String]) -> String {
    trace
        .iter()
        .enumerate()
        .map(|(index, line)| format!("  {index:2}. {line}\n"))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn submit_one(
    ring: &mut IoRing,
    run: &mut Run,
    op: &GenOp,
    handle: std::os::windows::io::RawHandle,
    shared: &SharedFile,
    registered_file: RegisteredFile,
    registered_buffers: &RegisteredBuffers<Vec<u8>>,
    coverage: &mut Coverage,
) {
    // A registered buffer already carrying a live operation cannot take
    // another: two operations writing one buffer is a data race in the
    // *generated program*, not a defect in the ring. Fall back to an owned
    // buffer when every index is busy rather than skipping the step.
    let free_index = (0..REGISTERED_BUFFERS)
        .find(|index| !run.buffer_in_use.values().any(|in_use| in_use == index));
    let buffer = match (op.buffer, free_index) {
        (BufferKind::Registered, Some(_)) => BufferKind::Registered,
        _ => BufferKind::Owned,
    };
    let span = free_index.map(|index| RegisteredSpan {
        buffer_index: index,
        offset: 0,
        len: BUF_LEN as u32,
    });

    // Claim-or-drop is a free axis only for an **owned** buffer. A registered
    // one is different by contract: `read_registered_raw`'s rustdoc states the
    // token "must be claimed once its completion is observed", because claiming
    // is the only thing that releases the use against `RegisteredBuffers`'s own
    // drop check. `Token`'s drop is deliberately empty (D-4), so dropping the
    // token instead leaks the `RegisteredUse`, permanently pins that buffer
    // index, and makes the whole registration undroppable. Generating that
    // would be emitting an invalid program and blaming the ring for it.
    let claim = op.claim || buffer == BufferKind::Registered;

    coverage.operations += 1;
    coverage.shapes.insert((op.kind, op.target, buffer));
    if op.buffer == BufferKind::Registered && buffer == BufferKind::Owned {
        coverage.buffer_downgrades += 1;
    }
    if !claim {
        coverage.deliberate_drops += 1;
    }

    let read_offset = (op.slot % 32) * BUF_LEN as u64;
    let write_offset = WRITE_BASE + (op.slot % 32) * BUF_LEN as u64;
    let options = PushOptions::new().drain_preceding(op.drain_preceding);

    let mut batch = Batch::new(ring);
    let held = match (op.kind, op.target, buffer) {
        (OpKind::Flush, TargetKind::Raw, _) => {
            // SAFETY: `handle` is the caller's file, open for the whole test.
            let user_data =
                unsafe { batch.flush_raw(handle, FlushCoverage::Unordered, FlushMode::Default) }
                    .expect("queue raw flush");
            batch.submit().expect("submit");
            // A raw flush carries no token, so nothing is ever owed for it.
            run.contract.observe_tokenless_push(user_data);
            return;
        }
        (OpKind::Flush, TargetKind::Shared, _) => batch
            .flush(shared, FlushCoverage::Unordered, FlushMode::Default)
            .map(Held::SharedFlush),
        (OpKind::Flush, TargetKind::Registered, _) => batch
            .flush(
                &registered_file,
                FlushCoverage::Unordered,
                FlushMode::Default,
            )
            .map(Held::RegdFlush),

        (OpKind::Read, TargetKind::Raw, BufferKind::Owned) => {
            // SAFETY: as above -- the handle outlives every operation, all of
            // which are drained before `run_plan` returns.
            unsafe { batch.read_raw(handle, vec![0_u8; BUF_LEN], read_offset, options) }
                .map(Held::RawOwned)
        }
        (OpKind::Read, TargetKind::Raw, BufferKind::Registered) => {
            // SAFETY: as above.
            unsafe {
                batch.read_registered_raw(
                    handle,
                    registered_buffers,
                    span.expect("a free registered buffer"),
                    read_offset,
                    options,
                )
            }
            .map(Held::RawRegistered)
        }
        (OpKind::Read, TargetKind::Shared, BufferKind::Owned) => batch
            .read(shared, vec![0_u8; BUF_LEN], read_offset, options)
            .map(Held::SharedOwned),
        (OpKind::Read, TargetKind::Shared, BufferKind::Registered) => batch
            .read_registered(
                shared,
                registered_buffers,
                span.expect("a free registered buffer"),
                read_offset,
                options,
            )
            .map(Held::SharedRegistered),
        (OpKind::Read, TargetKind::Registered, BufferKind::Owned) => batch
            .read(&registered_file, vec![0_u8; BUF_LEN], read_offset, options)
            .map(Held::RegdOwned),
        (OpKind::Read, TargetKind::Registered, BufferKind::Registered) => batch
            .read_registered(
                &registered_file,
                registered_buffers,
                span.expect("a free registered buffer"),
                read_offset,
                options,
            )
            .map(Held::RegdRegistered),

        (OpKind::Write, TargetKind::Raw, BufferKind::Owned) => {
            // SAFETY: as above.
            unsafe {
                batch.write_raw(
                    handle,
                    vec![7_u8; BUF_LEN],
                    write_offset,
                    options,
                    WriteCaching::Cached,
                )
            }
            .map(Held::RawOwned)
        }
        (OpKind::Write, TargetKind::Raw, BufferKind::Registered) => {
            // SAFETY: as above.
            unsafe {
                batch.write_registered_raw(
                    handle,
                    registered_buffers,
                    span.expect("a free registered buffer"),
                    write_offset,
                    options,
                    WriteCaching::Cached,
                )
            }
            .map(Held::RawRegistered)
        }
        (OpKind::Write, TargetKind::Shared, BufferKind::Owned) => batch
            .write(
                shared,
                vec![7_u8; BUF_LEN],
                write_offset,
                options,
                WriteCaching::Cached,
            )
            .map(Held::SharedOwned),
        (OpKind::Write, TargetKind::Shared, BufferKind::Registered) => batch
            .write_registered(
                shared,
                registered_buffers,
                span.expect("a free registered buffer"),
                write_offset,
                options,
                WriteCaching::Cached,
            )
            .map(Held::SharedRegistered),
        (OpKind::Write, TargetKind::Registered, BufferKind::Owned) => batch
            .write(
                &registered_file,
                vec![7_u8; BUF_LEN],
                write_offset,
                options,
                WriteCaching::Cached,
            )
            .map(Held::RegdOwned),
        (OpKind::Write, TargetKind::Registered, BufferKind::Registered) => batch
            .write_registered(
                &registered_file,
                registered_buffers,
                span.expect("a free registered buffer"),
                write_offset,
                options,
                WriteCaching::Cached,
            )
            .map(Held::RegdRegistered),
    };

    let held = held.expect("queue generated operation");
    let user_data = match &held {
        Held::RawOwned(t) => t.id(),
        Held::RawRegistered(t) => t.id(),
        Held::SharedOwned(t) => t.id(),
        Held::SharedRegistered(t) => t.id(),
        Held::SharedFlush(t) => t.id(),
        Held::RegdOwned(t) => t.id(),
        Held::RegdRegistered(t) => t.id(),
        Held::RegdFlush(t) => t.id(),
    };
    batch.submit().expect("submit");

    run.contract.observe_push(user_data);
    run.trace.push(format!(
        "{:?} target={:?} buffer={:?} {} drain_preceding={} offset={} user_data={user_data}",
        op.kind,
        op.target,
        buffer,
        if claim { "claim" } else { "DROP" },
        op.drain_preceding,
        if op.kind == OpKind::Write {
            write_offset
        } else {
            read_offset
        }
    ));
    if buffer == BufferKind::Registered
        && let Some(span) = span
    {
        run.buffer_in_use.insert(user_data, span.buffer_index);
    }
    if claim {
        run.held.insert(user_data, held);
    } else {
        // Dropped on purpose: the token's own `Drop` keeps whatever the kernel
        // still needs alive, and the contract is told so it is not reported as
        // an unstated leak.
        drop(held);
        run.dropped.insert(user_data, ());
    }
}

// --- the test ----------------------------------------------------------------

#[test]
fn generated_sequences_satisfy_the_ring_contract() {
    let seed = resolve_seed();
    ALLOC.announce_seed();
    eprintln!(
        "generated_sequences: seed 0x{seed:016X}\n\
         replay this run with:\n  \
         $env:{SEED_VAR}='0x{seed:016X}'; $env:WINDOWS_GUARD_ALLOC_SEED='0x{:016X}'; \
         cargo test -p windows-ioring-sys --test generated_sequences",
        ALLOC.seed()
    );
    assert!(
        ALLOC.total_allocations() > 0,
        "the guard-page allocator is not installed, so these sequences are running \
         uninstrumented and only prove that nothing crashed"
    );

    let (file, path) = fixture("sweep");
    let shared = SharedFile::new(duplicate_handle(&file));
    let mut rng = Rng(seed);
    let mut coverage = Coverage::default();

    for sequence in 0..SEQUENCES {
        let plan = generate(&mut rng);
        if let Err(failure) = run_plan(&plan, &file, &shared, &mut coverage) {
            panic!(
                "sequence {sequence} of {SEQUENCES} violated the ring contract\n\
                 seed: 0x{seed:016X} (replay: $env:{SEED_VAR}='0x{seed:016X}')\n\
                 guard allocator seed: 0x{:016X}\n\
                 {failure}",
                ALLOC.seed()
            );
        }
    }

    // D-41's corollary, applied to the generator itself: a green run proves
    // nothing unless it also shows which states it reached. Every shape below
    // is one this file advertises sampling, so a change that silently stopped
    // producing one -- registered buffers always downgrading, say -- fails here
    // rather than passing with a third of the space quietly untested.
    let mut missing = Vec::new();
    for kind in [OpKind::Read, OpKind::Write] {
        for target in [TargetKind::Raw, TargetKind::Shared, TargetKind::Registered] {
            for buffer in [BufferKind::Owned, BufferKind::Registered] {
                if !coverage.shapes.contains(&(kind, target, buffer)) {
                    missing.push(format!("{kind:?}/{target:?}/{buffer:?}"));
                }
            }
        }
    }
    for target in [TargetKind::Raw, TargetKind::Shared, TargetKind::Registered] {
        if !coverage
            .shapes
            .contains(&(OpKind::Flush, target, BufferKind::Owned))
        {
            missing.push(format!("Flush/{target:?}"));
        }
    }
    assert!(
        missing.is_empty(),
        "seed 0x{seed:016X} produced {} operations but never exercised: {}\n\
         the generator is sampling less of the space than this file claims",
        coverage.operations,
        missing.join(", ")
    );
    assert!(
        coverage.deliberate_drops > 0,
        "no token was ever dropped without claiming, so the claim-or-drop axis was not sampled"
    );
    assert!(
        coverage.mid_sequence_drains > 0,
        "no sequence ever drained mid-flight, so the drain-now-or-later axis was not sampled"
    );
    assert!(
        coverage.attaches_with_work_outstanding > 0,
        "the completion event was attached {} times but never to a ring with work outstanding, \
         so #47's own axis was not sampled",
        coverage.attaches
    );

    eprintln!(
        "generated_sequences: {} sequences, {} operations, {} distinct shapes, \
         {} deliberate drops, {} mid-sequence drains, {} attaches ({} with work outstanding), \
         {} registered-buffer downgrades",
        SEQUENCES,
        coverage.operations,
        coverage.shapes.len(),
        coverage.deliberate_drops,
        coverage.mid_sequence_drains,
        coverage.attaches,
        coverage.attaches_with_work_outstanding,
        coverage.buffer_downgrades
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}

// --- the regression corpus (M17.5) -------------------------------------------
//
// A generated run samples. A corpus does not: every entry here replays exactly,
// on every run, with no seed involved. That difference is the whole point --
// the generator reaches #47's shape within a handful of sequences on most
// seeds, but "on most seeds" is not a guarantee, and a shape that only some
// runs exercise is a shape that some runs do not.
//
// **Entries are added when a sequence is found, not when it is convenient.**
// The corpus is currently seeded from M17.4's calibration rather than from a
// live defect, because the generator has not found one in shipping code. That
// is not a reason to leave the mechanism unbuilt: the first real failure is
// exactly the moment nobody wants to be inventing a corpus format, and a
// property test that finds a bug and then forgets it has bought one debugging
// session rather than a guarantee.

/// A sequence that must keep passing, replayed verbatim rather than generated.
struct Regression {
    /// What went wrong, in the terms of the defect rather than the mechanism.
    why: &'static str,
    steps: Vec<Step>,
}

fn read(target: TargetKind, buffer: BufferKind, slot: u64) -> Step {
    Step::Op(GenOp {
        kind: OpKind::Read,
        target,
        buffer,
        claim: true,
        drain_preceding: false,
        slot,
    })
}

fn write(target: TargetKind, buffer: BufferKind, slot: u64) -> Step {
    Step::Op(GenOp {
        kind: OpKind::Write,
        target,
        buffer,
        claim: true,
        drain_preceding: false,
        slot,
    })
}

/// [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47), as the
/// generator reported it during M17.4's calibration.
///
/// Recorded in the shape it was actually found in rather than a tidied-up
/// minimal one: the essential part is the operation *before* the attach, which
/// leaves the completion queue non-empty at handover so that only
/// [D-20](DESIGN-NOTES.md#d-20)'s deliberate setup signal can wake a waiter --
/// but a corpus that quietly rewrites what it was given is a corpus nobody can
/// trust to have preserved the failing case.
fn issue_47_backlog_at_handover() -> Regression {
    Regression {
        why: "a completion queued before the event was attached left a waiter with no wakeup, \
              because the queue never returns to empty to re-arm the edge (D-19) and only D-20's \
              deliberate setup signal covers the backlog",
        steps: vec![
            write(TargetKind::Registered, BufferKind::Registered, 4),
            Step::Attach,
            read(TargetKind::Registered, BufferKind::Owned, 1),
            read(TargetKind::Registered, BufferKind::Registered, 1),
            read(TargetKind::Registered, BufferKind::Registered, 5),
        ],
    }
}

fn replay(tag: &str, regression: &Regression) {
    let (file, path) = fixture(tag);
    let shared = SharedFile::new(duplicate_handle(&file));
    let mut coverage = Coverage::default();
    let plan = Plan {
        steps: regression.steps.clone(),
    };

    let outcome = run_plan(&plan, &file, &shared, &mut coverage);

    drop(file);
    let _ = std::fs::remove_file(&path);

    if let Err(failure) = outcome {
        panic!(
            "regression `{tag}` reproduced a defect that was fixed\n\
             what this sequence caught when it was found: {}\n\
             {failure}",
            regression.why
        );
    }
}

#[test]
fn regression_issue_47_backlog_at_handover() {
    replay("issue-47", &issue_47_backlog_at_handover());
}

// --- guarding the detector itself --------------------------------------------

/// The lost-wakeup detector must actually fire, and this proves it without
/// reverting anything in the crate.
///
/// M17.4 established by hand that [`wait_then_drain`] catches #47, by removing
/// D-20's setup signal and watching the generator go red. That procedure cannot
/// run in CI -- it edits `src/` -- so nothing automated would notice if the wait
/// were later "simplified" back into a poll, which is exactly the change that
/// made the generator blind in the first place (`D-42`).
///
/// This constructs the lost-wakeup condition against a *correct* ring instead:
/// attach, consume the setup signal, then wait again **without draining**. The
/// queue is non-empty and nothing new can complete, so no empty-to-non-empty
/// edge can occur and no wakeup is owed -- the same observable state a real lost
/// wakeup produces. A detector that reports nothing here reports nothing for
/// #47 either.
#[test]
fn the_lost_wakeup_detector_fires_when_no_wakeup_is_owed() {
    /// Short, because this wait is *expected* to expire; the full `WAIT_MS`
    /// would add five seconds to every run to learn the same thing.
    const EXPECT_TIMEOUT_MS: u32 = 250;

    let (file, path) = fixture("detector");
    let handle = file.as_raw_handle();
    let mut ring = IoRing::new(64, 64).expect("create ring");
    let mut run = Run {
        contract: RingContract::new(),
        held: HashMap::new(),
        dropped: HashMap::new(),
        buffer_in_use: HashMap::new(),
        trace: Vec::new(),
    };

    {
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `file` outlives every operation queued here -- all of them are
        // drained before this test returns.
        let token = unsafe { batch.read_raw(handle, vec![0_u8; BUF_LEN], 0, PushOptions::new()) }
            .expect("queue read");
        run.contract.observe_push(token.id());
        run.held.insert(token.id(), Held::RawOwned(token));
        batch.submit().expect("submit");
    }

    let event = ring.completion_event().expect("attach completion event");
    assert!(
        signalled_within(&event, WAIT_MS),
        "attaching must raise the deliberate setup signal (D-20); without it this test would \
         pass for the wrong reason"
    );

    // Deliberately do not drain. The completion is sitting in a non-empty queue,
    // so the edge cannot re-arm and no further wakeup is owed.
    let outcome = wait_then_drain(&mut ring, &mut run, Some(&event), EXPECT_TIMEOUT_MS);
    let report = outcome.expect_err(
        "the detector reported no lost wakeup while waiting on a queue that cannot signal again \
         -- it would be equally silent for #47",
    );
    assert!(
        report.contains("a wakeup was lost"),
        "the detector fired but did not say why: {report}"
    );

    // Leave the ring quiet so it can be dropped: the completion is still there.
    drain_to_empty(&mut ring, &mut run);
    run.contract.assert_quiescent();

    drop(file);
    let _ = std::fs::remove_file(&path);
}
