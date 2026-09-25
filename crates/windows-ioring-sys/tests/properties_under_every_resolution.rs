// Copyright (c) Mike Grier
//! The properties that must hold under **every** resolution (M26.4).
//!
//! `M26.3` built a resolver that chooses a point in
//! [RESPONSE-SPACE.md](../RESPONSE-SPACE.md) from a seed. This file is what
//! that resolver is *for*: five properties this crate must satisfy whichever
//! point is chosen, driven across many plans and many resolutions.
//!
//! | # | Property | Checked by |
//! |---|---|---|
//! | P-1 | Conservation: no lost, duplicated or unclaimed completion | [`RingContract`], fed from the run |
//! | P-2 | No hang: every loop terminates | a step budget, exceeded = failure |
//! | P-3 | [`IoRing::pop_within`] honours its bound | elapsed time, plus a stalled resolution |
//! | P-4 | [`IoRing::outstanding`] is accurate | compared against the contract's own count |
//! | P-5 | No use-after-free | [`windows_guard_alloc::GuardAlloc`] |
//!
//! # `RingContract` is the definition, not a second copy
//!
//! P-1 is not restated here. [`RingContract`] already holds this crate's
//! conservation rules as an oracle over observed sequences, and the layer that
//! owns an invariant owns the oracle for it -- a copy written in a harness is
//! a second implementation of the rule rather than a check of it, and when the
//! two disagree it is the harness that gets "fixed". So this file *reports* to
//! the contract and asks it for the verdict.
//!
//! P-4 follows the same rule rather than counting for itself. "How many
//! operations are outstanding" is a fact the contract already derives from
//! what it was told, so the expected value is read back out of it via
//! [`Violation::Outstanding`] rather than tracked in a counter beside it. A
//! counter here would be a third party to the disagreement.
//!
//! # What these properties are honestly weak about
//!
//! Stated plainly, because a property suite that oversells itself is worse
//! than one that admits a gap -- the gap is then invisible rather than merely
//! open.
//!
//! **P-3's upper bound is nearly free under an ordinary resolution.** The
//! resolver answers a wait immediately, so `pop_within` rarely approaches its
//! deadline and "it did not exceed the bound" is close to vacuous. The
//! non-vacuous case needs a resolution in which *nothing completes during the
//! window*, which is what [`Stalled`] supplies -- and note what that is and is
//! not: it is not a violation of `RS-C-1`, which says every operation
//! completes *eventually*, because no finite observation can distinguish
//! "eventually" from "never". It is the prefix of an `RS-C-1`-satisfying
//! resolution in which the eventually has not happened yet, and that prefix is
//! exactly when a bound has to be honoured.
//!
//! **P-5 covers this crate's memory handling, not the kernel's.** Under a
//! resolver no operation reaches the kernel, so nothing external writes into a
//! buffer and the classic ring use-after-free -- the kernel writing through a
//! pointer whose owner has dropped -- cannot occur here at all. What the guard
//! allocator still sees is every lifetime decision the crate itself makes
//! around tokens, buffers and guards. The kernel-side half is
//! [generated_sequences.rs](generated_sequences.rs)'s job, against a real
//! ring, and stays there.
//!
//! # Three seeds, and why they are not one
//!
//! `D-41`'s discipline is that one number replays one thing.
//! [generated_sequences.rs](generated_sequences.rs) already carries two axes;
//! this adds the resolver's, making three. They are independent on purpose:
//! pinning the plan seed alone reproduces the same operations against
//! different resolutions, which is what you want when a plan looks suspicious,
//! and pinning the resolver seed alone reproduces the same resolutions against
//! different plans. A failure prints all three, because only all three replay
//! the whole run.

#![cfg(all(windows, feature = "kernel-seam"))]

use std::time::{Duration, Instant};

use windows_ioring_sys::contract::{RingContract, Violation};
use windows_ioring_sys::sys::{Resolver, ResolverWatch, Responses};
use windows_ioring_sys::{
    Batch, Completion, FlushCoverage, FlushMode, IoRing, PushOptions, SharedFile, Token,
    WriteCaching,
};
use windows_sys::core::HRESULT;

/// P-5. A use-after-free in a generated plan must fault rather than read stale
/// bytes, or this file is only checking that nothing happened to crash.
#[global_allocator]
static ALLOC: windows_guard_alloc::GuardAlloc = windows_guard_alloc::GuardAlloc::new();

/// Environment override for the plan seed (decimal, or `0x` hex). The resolver
/// and the guard allocator each have their own, separate variable.
const PLAN_SEED_VAR: &str = "WINDOWS_IORING_PROPERTY_SEED";

/// Plans per run, and the longest plan generated.
///
/// Sized so that every step kind and every operation shape appears many times
/// over, while the whole file stays quick enough that nobody is tempted to
/// skip it. No real I/O happens under a resolver, so the cost is almost
/// entirely the ring handles.
const PLANS: usize = 240;
/// Steps in the longest plan.
const MAX_STEPS: usize = 10;
/// Operations in the largest single batch.
const MAX_BATCH: usize = 5;

/// P-2's budget: how many consultations one plan may take to quiesce.
///
/// This is the whole of "no hang" as a finite test can state it. A resolver
/// answers instantly, so a genuine hang here is an unterminating *loop* rather
/// than a block, and a loop that exceeds a budget generous enough for any
/// legitimate resolution is the observable form of one. Generous is load
/// bearing: a tight budget would fail for slow-but-correct resolutions and
/// teach its reader to raise it.
const DRAIN_BUDGET: usize = 4096;

/// Bytes per write. Small, because the buffer exists to give the guard
/// allocator a lifetime to watch rather than to move data.
const BUF_LEN: usize = 256;

// ------------------------------------------------------------- generation ---

/// SplitMix64, the generator this crate uses everywhere (`D-41`).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// One operation in a batch.
#[derive(Debug, Clone, Copy)]
enum Op {
    Flush { drain: bool },
    Write { drain: bool },
}

/// One step of a plan.
#[derive(Debug, Clone)]
enum Step {
    /// Build a batch and submit it.
    Submit(Vec<Op>),
    /// Poll `try_pop` until the queue reports empty.
    Drain,
    /// Wait for one completion with a bound.
    PopWithin(u64),
    /// Check P-4 at an arbitrary mid-run point rather than only at the end.
    CheckOutstanding,
}

fn generate(rng: &mut Rng) -> Vec<Step> {
    let steps = 1 + rng.below(MAX_STEPS as u64) as usize;
    (0..steps)
        .map(|_| match rng.below(10) {
            0..=4 => {
                let count = 1 + rng.below(MAX_BATCH as u64) as usize;
                Step::Submit(
                    (0..count)
                        .map(|_| {
                            let drain = rng.chance(25);
                            if rng.chance(50) {
                                Op::Flush { drain }
                            } else {
                                Op::Write { drain }
                            }
                        })
                        .collect(),
                )
            }
            5..=6 => Step::Drain,
            7..=8 => Step::PopWithin(rng.below(20)),
            _ => Step::CheckOutstanding,
        })
        .collect()
}

// ------------------------------------------------------------------- run ---

/// Tokens still awaiting their completion.
///
/// Two lists because the two operations hand back different types, and that
/// difference is the point: a write's token owns its buffer, which is the
/// lifetime P-5 is watching.
#[derive(Default)]
struct Outstanding {
    flushes: Vec<Token<SharedFile>>,
    writes: Vec<Token<(Vec<u8>, SharedFile)>>,
}

/// What a whole run actually exercised.
///
/// Without this the suite could pass by doing nothing: a generator that
/// produced empty plans, or a resolution that completed everything inside the
/// submit that carried it, would satisfy all five properties trivially. These
/// counters are asserted at the end, so "the properties held" means "the
/// properties held over runs that reached the states they are about".
#[derive(Debug, Default)]
struct Coverage {
    pushes: usize,
    completions: usize,
    claims: usize,
    deliberate_leaks: usize,
    barriers: usize,
    writes: usize,
    flushes: usize,
    declined_submits: usize,
    /// `CheckOutstanding` steps that ran with work genuinely in flight -- the
    /// only ones where P-4 could have caught a disagreement.
    outstanding_checks_with_work: usize,
    /// `PopWithin` calls that found nothing, which is the case P-3's bound is
    /// about.
    empty_pop_withins: usize,
}

struct Run {
    ring: IoRing,
    file: SharedFile,
    contract: RingContract,
    tokens: Outstanding,
    /// The resolution's own record, so a declined submit can be recognised by
    /// asking the resolver rather than by matching an `HRESULT` here.
    watch: ResolverWatch,
    /// Declined submits this plan absorbed, for the trace.
    ///
    /// The thinnest of the coverage signals, because a decline has to fall on
    /// a plan that then reaches a `pop_within` -- a decline absorbed by the
    /// ignored `batch.submit()` is never counted here. Its threshold is only
    /// "more than none" for that reason, and tightening it would buy a flake
    /// rather than a guarantee.
    declined: usize,
    /// Plans print their steps only on failure, so this costs nothing until
    /// something needs reading.
    trace: Vec<String>,
    coverage: Coverage,
}

impl Coverage {
    fn fold(&mut self, other: &Self) {
        self.pushes += other.pushes;
        self.completions += other.completions;
        self.claims += other.claims;
        self.deliberate_leaks += other.deliberate_leaks;
        self.barriers += other.barriers;
        self.writes += other.writes;
        self.flushes += other.flushes;
        self.declined_submits += other.declined_submits;
        self.outstanding_checks_with_work += other.outstanding_checks_with_work;
        self.empty_pop_withins += other.empty_pop_withins;
    }
}

/// P-4's expected value, derived from the contract rather than counted here.
///
/// An operation the contract still has in a pushed state is exactly one whose
/// completion has not been popped, which is what `IoRing::outstanding` counts.
fn contract_outstanding(contract: &RingContract) -> usize {
    contract
        .check_quiescent()
        .iter()
        .filter(|violation| matches!(violation, Violation::Outstanding { .. }))
        .count()
}

impl Run {
    fn new(path: &std::path::Path, watch: ResolverWatch) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)?;
        Ok(Self {
            ring: IoRing::new(64, 128)?,
            file: SharedFile::new(file.into()),
            contract: RingContract::new(),
            tokens: Outstanding::default(),
            watch,
            declined: 0,
            trace: Vec::new(),
            coverage: Coverage::default(),
        })
    }

    /// One `pop_within`, absorbing a submit the resolution declined.
    ///
    /// **A declined submit is not a property failure**, and treating it as one
    /// was this harness's own first defect. `RS-P-7` leaves the operations
    /// queued rather than losing them, and `pop_within` documents that it
    /// returns any error from `SubmitIoRing` -- so a correct consumer retries,
    /// and a harness that panicked instead would be asserting a contract the
    /// crate never offered.
    ///
    /// Which errors are refusals is **asked of the resolver** rather than
    /// matched against an `HRESULT` here. A hard-coded code would be a second
    /// copy of a choice the resolver owns, and the two would drift the first
    /// time it picked a different one. Retrying is bounded by P-2's budget in
    /// every caller, so a resolution that declined forever is still caught.
    fn pop_within(&mut self, bound: Duration) -> Result<Option<Completion>, String> {
        let before = self.watch.stats().failed_submits;
        match self.ring.pop_within(bound) {
            Ok(outcome) => Ok(outcome),
            Err(_) if self.watch.stats().failed_submits > before => {
                self.declined += 1;
                self.coverage.declined_submits += 1;
                Ok(None)
            }
            Err(error) => Err(format!("pop_within failed: {error}")),
        }
    }

    /// P-4, checked wherever it is asked for.
    fn check_outstanding(&mut self, where_: &str) -> Result<(), String> {
        let expected = contract_outstanding(&self.contract);
        let actual = self.ring.outstanding();
        if expected == actual {
            if expected > 0 {
                self.coverage.outstanding_checks_with_work += 1;
            }
            self.trace.push(format!("{where_}: outstanding {actual}"));
            return Ok(());
        }
        Err(format!(
            "P-4 broken at {where_}: IoRing::outstanding() says {actual}, the contract's own \
             record says {expected}"
        ))
    }

    /// Account for one popped completion: tell the contract, then either claim
    /// the token or abandon it on purpose.
    ///
    /// The order matters and is not arbitrary. `RingContract` moves a `Pushed`
    /// operation to a provisionally-leaked state on completion and only
    /// `observe_claim` corrects it, so reporting a deliberate leak *before* the
    /// completion would leave the contract seeing a second completion for an
    /// already-settled operation and reporting a duplicate that did not happen.
    fn account(&mut self, completion: &Completion, rng: &mut Rng) -> Result<(), String> {
        let user_data = completion.user_data();
        self.contract.observe_completion(user_data);
        self.coverage.completions += 1;

        if let Some(at) = self
            .tokens
            .flushes
            .iter()
            .position(|token| token.id() == user_data)
        {
            let token = self.tokens.flushes.remove(at);
            return self.settle(token.claim_if(completion).is_ok(), user_data, rng);
        }
        if let Some(at) = self
            .tokens
            .writes
            .iter()
            .position(|token| token.id() == user_data)
        {
            let token = self.tokens.writes.remove(at);
            return self.settle(token.claim_if(completion).is_ok(), user_data, rng);
        }
        Err(format!(
            "a completion arrived for {user_data:#x}, which no live token matches -- either \
             RS-C-2 was broken or this harness lost a push"
        ))
    }

    fn settle(&mut self, claimed: bool, user_data: usize, rng: &mut Rng) -> Result<(), String> {
        if !claimed {
            return Err(format!(
                "the token for {user_data:#x} refused a completion carrying its own identity, \
                 which is RS-C-2 broken inside the crate's own matching"
            ));
        }
        // Claiming already happened; whether to *report* it as a claim or as a
        // deliberate abandonment is what exercises both of the contract's
        // settled states. Both are legitimate endings, and the contract
        // distinguishes them precisely so an unstated leak stays a violation.
        if rng.chance(15) {
            self.contract.observe_deliberate_leak(user_data);
            self.coverage.deliberate_leaks += 1;
        } else {
            self.contract.observe_claim(user_data);
            self.coverage.claims += 1;
        }
        Ok(())
    }

    /// Pop everything currently available. P-2's budget applies.
    fn drain(&mut self, rng: &mut Rng) -> Result<(), String> {
        for _ in 0..DRAIN_BUDGET {
            let Some(completion) = self
                .ring
                .try_pop()
                .map_err(|error| format!("try_pop failed: {error}"))?
            else {
                return Ok(());
            };
            self.account(&completion, rng)?;
        }
        Err(format!(
            "P-2 broken: try_pop kept yielding completions past a budget of {DRAIN_BUDGET}, \
             which no resolution satisfying RS-C-1 can do"
        ))
    }

    /// Drive to quiescence. P-2's budget applies, and this is where a
    /// resolution that stopped making progress is caught.
    fn quiesce(&mut self, rng: &mut Rng) -> Result<(), String> {
        for _ in 0..DRAIN_BUDGET {
            if self.ring.outstanding() == 0 {
                self.drain(rng)?;
                return Ok(());
            }
            match self.pop_within(Duration::from_millis(5))? {
                Some(completion) => self.account(&completion, rng)?,
                None => continue,
            }
        }
        Err(format!(
            "P-2 broken: {} operations still outstanding after a budget of {DRAIN_BUDGET} \
             consultations, so this resolution never satisfied RS-C-1",
            self.ring.outstanding()
        ))
    }
}

/// Run one plan under one resolution. `Ok` is every property holding.
fn run_plan(plan: &[Step], plan_rng: &mut Rng, run: &mut Run) -> Result<(), String> {
    for (index, step) in plan.iter().enumerate() {
        run.trace.push(format!("{index}: {step:?}"));
        match step {
            Step::Submit(ops) => {
                let mut pushed = Vec::new();
                {
                    let mut batch = Batch::new(&mut run.ring);
                    for op in ops {
                        match op {
                            Op::Flush { drain } => {
                                let coverage = if *drain {
                                    FlushCoverage::CoversPrecedingOperations
                                } else {
                                    FlushCoverage::Unordered
                                };
                                let token = batch
                                    .flush(&run.file, coverage, FlushMode::Default)
                                    .map_err(|error| format!("flush build failed: {error}"))?;
                                pushed.push((token.id(), *drain, false));
                                run.tokens.flushes.push(token);
                            }
                            Op::Write { drain } => {
                                let token = batch
                                    .write(
                                        &run.file,
                                        vec![0_u8; BUF_LEN],
                                        0,
                                        PushOptions::new().drain_preceding(*drain),
                                        WriteCaching::Cached,
                                    )
                                    .map_err(|error| format!("write build failed: {error}"))?;
                                pushed.push((token.id(), *drain, true));
                                run.tokens.writes.push(token);
                            }
                        }
                    }
                    // A submit may be declined under RS-P-7, which leaves the
                    // operations queued for a later one rather than losing
                    // them -- so it is not an error here, and the properties
                    // below still have to hold. That the refusal reaches
                    // `run_down` as a failure is a separate, queued finding
                    // (M26.8); this path never calls it.
                    let _ = batch.submit();
                }
                // Reported only after the batch is gone, because a push is
                // reported when it *queued*, and the build is what queues it.
                for (user_data, drain, is_write) in pushed {
                    run.contract.observe_push(user_data);
                    run.coverage.pushes += 1;
                    if drain {
                        run.coverage.barriers += 1;
                    }
                    if is_write {
                        run.coverage.writes += 1;
                    } else {
                        run.coverage.flushes += 1;
                    }
                }
            }
            Step::Drain => run.drain(plan_rng)?,
            Step::PopWithin(millis) => {
                // P-3. The bound is what is being checked, so it is measured
                // around the call rather than assumed from the argument.
                let bound = Duration::from_millis(*millis);
                let started = Instant::now();
                let outcome = run.pop_within(bound)?;
                let elapsed = started.elapsed();
                if elapsed > bound + Duration::from_millis(250) {
                    return Err(format!(
                        "P-3 broken: pop_within({millis}ms) took {elapsed:?}"
                    ));
                }
                if let Some(completion) = outcome {
                    run.account(&completion, plan_rng)?;
                } else {
                    run.coverage.empty_pop_withins += 1;
                }
            }
            Step::CheckOutstanding => run.check_outstanding(&format!("step {index}"))?,
        }
    }

    run.quiesce(plan_rng)?;
    run.check_outstanding("quiescence")?;

    // P-1, asked of the oracle rather than restated.
    let violations = run.contract.check_quiescent();
    if !violations.is_empty() {
        return Err(format!(
            "P-1 broken: the ring contract reports\n{}",
            violations
                .iter()
                .map(|violation| format!("    - {violation}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(())
}

fn seed_from_env(var: &str) -> u64 {
    match std::env::var(var) {
        Ok(text) => {
            let trimmed = text.trim();
            trimmed
                .strip_prefix("0x")
                .or_else(|| trimmed.strip_prefix("0X"))
                .map_or_else(
                    || trimmed.parse::<u64>().ok(),
                    |hex| u64::from_str_radix(hex, 16).ok(),
                )
                .unwrap_or_else(|| {
                    panic!("{var} is set to {text:?}, which is neither decimal nor 0x hex")
                })
        }
        Err(_) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x243F_6A88_85A3_08D3, |elapsed| elapsed.as_nanos() as u64),
    }
}

#[test]
fn the_properties_hold_under_every_resolution() {
    let plan_seed = seed_from_env(PLAN_SEED_VAR);
    let resolver_seed = seed_from_env(windows_ioring_sys::sys::RESOLVER_SEED_VAR);
    ALLOC.announce_seed();
    eprintln!(
        "properties: plan seed 0x{plan_seed:016X}, resolver seed 0x{resolver_seed:016X}\n\
         replay this run with:\n  \
         $env:{PLAN_SEED_VAR}='0x{plan_seed:016X}'; \
         $env:{}='0x{resolver_seed:016X}'; \
         $env:WINDOWS_GUARD_ALLOC_SEED='0x{:016X}'; \
         cargo test -p windows-ioring-sys --all-features --test properties_under_every_resolution",
        windows_ioring_sys::sys::RESOLVER_SEED_VAR,
        ALLOC.seed()
    );
    assert!(
        ALLOC.total_allocations() > 0,
        "P-5's detector is not installed, so these plans are running uninstrumented and only \
         prove that nothing crashed"
    );

    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-m26-4-{}.tmp",
        std::process::id()
    ));
    let mut plan_rng = Rng(plan_seed);
    let mut resolver_rng = Rng(resolver_seed);
    let mut coverage = Coverage::default();

    for index in 0..PLANS {
        let plan = generate(&mut plan_rng);
        // The resolution is drawn from its own axis. Deriving it from the plan
        // seed would make the two inseparable, and pinning either would then
        // reproduce a run that is half the one that failed.
        let this_resolver = resolver_rng.next();
        let resolver = Resolver::new(this_resolver);

        let outcome = resolver.scoped(|watch| {
            let mut run = Run::new(&path, watch.clone()).expect("a ring and a scratch file");
            let outcome = run_plan(&plan, &mut plan_rng, &mut run);
            // Whatever happened, take the ring down inside the scope -- its
            // rundown is answered by the resolver that owns its operations.
            // Its result is not checked: `quiesce` has already driven
            // everything to completion, so this is a no-op on the passing
            // path, and on the failing path a rundown error would only
            // re-report `M26.8` over whatever actually went wrong.
            let _ = run.ring.run_down();
            match outcome {
                Ok(()) => Ok(run.coverage),
                Err(failure) => Err((failure, run.declined, run.trace)),
            }
        });

        match outcome {
            Ok(plan_coverage) => coverage.fold(&plan_coverage),
            Err((failure, declined, trace)) => panic!(
                "plan {index} of {PLANS} broke a property\n\
                 replay: $env:{PLAN_SEED_VAR}='0x{plan_seed:016X}'; \
                 $env:{}='0x{resolver_seed:016X}'; \
                 $env:WINDOWS_GUARD_ALLOC_SEED='0x{:016X}'\n\
                 this plan's resolution: 0x{this_resolver:016X}\n\
                 declined submits absorbed: {declined}\n\
                 {failure}\n\
                 trace:\n  {}",
                windows_ioring_sys::sys::RESOLVER_SEED_VAR,
                ALLOC.seed(),
                trace.join("\n  ")
            ),
        }
    }
    let _ = std::fs::remove_file(&path);

    // D-41's corollary, applied to this suite: a green run is evidence only if
    // the run reached the states the properties are about. Each of these would
    // be zero for a generator that produced empty plans, a resolution that
    // resolved everything inside its submit, or a harness that quietly stopped
    // reporting -- all of which pass every assertion above.
    eprintln!("coverage: {coverage:?}");
    assert!(coverage.pushes > 500, "too few operations: {coverage:?}");
    assert_eq!(
        coverage.completions, coverage.pushes,
        "every pushed operation must have completed exactly once for the run to have finished \
         at all -- {coverage:?}"
    );
    assert!(
        coverage.writes > 100 && coverage.flushes > 100,
        "both operation shapes must appear: {coverage:?}"
    );
    assert!(
        coverage.barriers > 50,
        "too few drain-flagged operations, so RS-C-4 barely applied: {coverage:?}"
    );
    assert!(
        coverage.claims > 100 && coverage.deliberate_leaks > 10,
        "both of the contract's settled states must be reached: {coverage:?}"
    );
    assert!(
        coverage.declined_submits > 0,
        "no resolution ever declined a submit, so RS-P-7 never reached the crate: {coverage:?}"
    );
    assert!(
        coverage.outstanding_checks_with_work > 25,
        "P-4 was only ever checked with an empty ring, where it cannot catch a disagreement: \
         {coverage:?}"
    );
    assert!(
        coverage.empty_pop_withins > 10,
        "pop_within always found something, so its bound never had anything to do: {coverage:?}"
    );
}

// ------------------------------------------------------------------- P-3 ---

/// A resolution in which nothing has completed yet.
///
/// Not a violation of `RS-C-1`, which says every operation completes
/// *eventually* -- no finite observation can distinguish "eventually" from
/// "never", so this is the prefix of a satisfying resolution rather than a
/// counter-example to one. That prefix is the only window in which a bound has
/// anything to do, which is why P-3's non-vacuous case needs it.
///
/// Deliberately not a [`Resolver`] with a switch: a knob that made a resolver
/// stop satisfying a constraint is exactly what `D-61` refuses, because a test
/// could then assert against a platform that cannot exist. This is a separate,
/// degenerate responder that claims to be nothing else.
struct Stalled;

impl Responses for Stalled {
    unsafe fn submit(
        &mut self,
        _ring: *mut std::ffi::c_void,
        _wait_operations: u32,
        _milliseconds: u32,
        submitted: *mut u32,
    ) -> HRESULT {
        // SAFETY: a valid out-pointer, as the Win32 call requires.
        unsafe { submitted.write(0) };
        0
    }

    unsafe fn pop(
        &mut self,
        _ring: *mut std::ffi::c_void,
        _cqe: *mut windows_sys::Win32::Storage::FileSystem::IORING_CQE,
    ) -> HRESULT {
        // `S_FALSE`: the queue is empty.
        1
    }

    unsafe fn build_flush(
        &mut self,
        _ring: *mut std::ffi::c_void,
        _file: windows_sys::Win32::Storage::FileSystem::IORING_HANDLE_REF,
        _mode: windows_sys::Win32::Storage::FileSystem::FILE_FLUSH_MODE,
        _user_data: usize,
        _flags: i32,
    ) -> HRESULT {
        0
    }
}

#[test]
fn pop_within_honours_its_bound_when_nothing_completes() {
    // P-3, in the only window where it is not nearly vacuous. Under an
    // ordinary resolution a completion is usually waiting almost at once, so
    // "it did not exceed the bound" says little; here nothing arrives at all
    // and the deadline is the only thing that can end the call.
    let guard = windows_ioring_sys::sys::install(Box::new(Stalled));
    let mut ring = IoRing::new(64, 128).expect("a ring");
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-m26-4-stalled-{}.tmp",
        std::process::id()
    ));
    let file = SharedFile::new(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .expect("a scratch file")
            .into(),
    );

    let token = {
        let mut batch = Batch::new(&mut ring);
        let token = batch
            .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
            .expect("a flush builds");
        batch.submit().expect("the submit is answered");
        token
    };
    assert_eq!(
        ring.outstanding(),
        1,
        "the bound only has work to do while something is outstanding -- with nothing \
         outstanding pop_within returns at once by documented design, which would make this \
         test measure the early return instead"
    );

    for millis in [0_u64, 5, 30, 120] {
        let bound = Duration::from_millis(millis);
        let started = Instant::now();
        let outcome = ring.pop_within(bound).expect("pop_within does not fail");
        let elapsed = started.elapsed();
        assert!(
            outcome.is_none(),
            "nothing completed, so nothing may be popped"
        );
        assert!(
            elapsed >= bound.saturating_sub(Duration::from_millis(20)),
            "pop_within({millis}ms) returned after {elapsed:?}, well short of its bound -- a \
             bound honoured by returning early is not honoured"
        );
        assert!(
            elapsed < bound + Duration::from_millis(400),
            "pop_within({millis}ms) took {elapsed:?}, past its bound"
        );
    }

    // The operation never completed, so the token is abandoned on purpose and
    // the ring is left to its own teardown; calling `run_down` here would park
    // forever, which is this responder working as described rather than a
    // defect to route around.
    std::mem::forget(token);
    std::mem::forget(ring);
    drop(guard);
    drop(file);
    let _ = std::fs::remove_file(&path);
}
