// Copyright (c) Mike Grier.
//! Diagnostic stress instruments for the flush-barrier contract.
//!
//! [`flush_barrier.rs`](flush_barrier.rs) asserts the contract once. These run
//! it many times, under deliberate contention, and keep enough of a record to
//! say *what happened* when an assertion fails -- because the failure this
//! exists to diagnose has so far only appeared under a loaded full-workspace
//! run, where the one thing available afterwards was `left: 1, right: 0`.
//!
//! # The question these were built to answer, and the answer they gave
//!
//! `FlushCoverage::CoversPrecedingOperations` sets
//! `IOSQE_FLAGS_DRAIN_PRECEDING_OPS`. [D-24](../DESIGN-NOTES.md#d-24) originally
//! recorded this as a **full ring-wide barrier**: preceding operations drain,
//! *and* operations pushed after it are held until it completes. On 2026-09-03
//! and again on 2026-09-06, one of 32 post-barrier writes was observed
//! completing *before* the flush, which looked like a flaky test.
//!
//! That admitted two readings, and a single assertion could not separate them:
//! either the documented guarantee was false, or the observation was not
//! faithful -- a test that drains, asserts and discards cannot show whether the
//! overtaking completion was posted in a different drain round, arrived alone or
//! in a burst, or how its timing compared to the flush's.
//!
//! **These instruments settled it.** Across roughly 4,500 trials the drain side
//! never failed once, the hold-back side failed in every condition including a
//! completely idle ring, and every overtake was confined to a single drain
//! round -- so the order observed is the order the kernel posted. The flag is
//! **one-sided**, and D-24's second half was withdrawn by
//! [D-47](../DESIGN-NOTES.md#d-47-detail).
//!
//! So read what follows in that light. The word "barrier" here means the drain
//! of what precedes, which holds; it does not mean subsequent work is held back,
//! which it is not. `phase_b_before_flush` is not a violation counter -- it
//! counts the *documented* one-sided behaviour, and the campaign reports the two
//! directions separately precisely because they have different answers.
//!
//! # What they are still for
//!
//! The contract is settled, so nothing here asserts the withdrawn half. They
//! remain because the evidence behind a shipped contract should be re-runnable
//! by someone who doubts it, or on a machine, device, or Windows build that
//! might behave differently. A future reader asking "is that still true here?"
//! should not have to rebuild the instrument to find out.
//!
//! # Why these are `#[ignore]`
//!
//! Each trial writes tens of MiB of unbuffered device I/O, and the contended
//! tests deliberately saturate a disk. That belongs nowhere near an ordinary
//! `cargo test`. Run them explicitly:
//!
//! ```text
//! cargo test -p windows-ioring-sys --test flush_barrier_stress -- --ignored --nocapture --test-threads 1
//! ```
//!
//! **`--test-threads 1` is not optional decoration.** Three of these instruments
//! each start four contention writers, so the default parallel harness runs
//! twelve of them at once and every instrument measures a load three times what
//! it asked for -- while also competing with the other instruments' actual
//! measurements. The numbers still come out, and they are not the numbers the
//! instrument is designed to produce.
//!
//! An earlier version of this line omitted the flag, which is also how a
//! temp-file collision between concurrent contention generators went unnoticed:
//! every verification run had been serial.
//!
//! [`tools/soak-flush-barrier.ps1`](../../../tools/soak-flush-barrier.ps1) runs
//! them one at a time for this reason, and is the better way to run a campaign.
//!
//! # An instrument's first report line is scraped
//!
//! [`tools/soak-flush-barrier.ps1`](../../../tools/soak-flush-barrier.ps1)
//! records the **first** line matching `D-23` as the detail column of its CSV.
//! So an instrument that prints several reports must print the representative
//! one **first**: an aggregate, then the breakdown.
//!
//! This is a real coupling between two files and it has already been wrong
//! twice -- the concurrent-rings instrument put worker 0's figures in a column
//! labelled with the whole instrument, and the depth sweep put depth 128's
//! there. Both are the same shape: a number that is quietly unrepresentative
//! rather than obviously missing.
//!
//! The trial budget comes from the environment so a run can be widened without
//! editing this file:
//!
//! ```text
//! IORING_STRESS_TRIALS=500 cargo test ... -- --ignored --nocapture --test-threads 1
//! ```
//!
//! It is a **budget, not a per-instrument count**: three instruments run
//! exactly that many trials and two subdivide it, so a round's total work is
//! not five times the number. See [`trials`] for the table and what it means
//! for reading a soak result or budgeting disk writes.
//!
//! # Why the fixture helpers are duplicated from `flush_barrier.rs`
//!
//! Deliberately, and it is the repository's duplicate-then-decide posture
//! rather than an oversight. These are *instruments* investigating whether that
//! test's assertion is sound; sharing a fixture would mean an edit made while
//! investigating could change the contract test's behaviour underneath it. When
//! the question is settled, either these are deleted or the fixture is factored
//! out once -- but not before.

#![cfg(windows)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, IoBuf, IoRing, PushOptions, Token, WriteCaching,
};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

// ---------------------------------------------------------------------------
// Fixture, duplicated from flush_barrier.rs on purpose (see the module docs).
// ---------------------------------------------------------------------------

const PHASE_OPS: usize = 32;
const BIG_LEN: usize = 1024 * 1024;
const SMALL_LEN: usize = 4096;
const ALIGN: usize = 4096;
const PHASE_B_BASE: usize = PHASE_OPS * BIG_LEN;
const WAIT_MS: u32 = 120_000;

/// Default trials per test. Overridden by `IORING_STRESS_TRIALS`.
///
/// Sized from the contract test's own cost: it runs two cases in about 0.16s,
/// so a hundred trials is seconds rather than minutes.
const DEFAULT_TRIALS: usize = 100;

/// The trial **budget**, from `IORING_STRESS_TRIALS`. Not a per-instrument count.
///
/// Three instruments run exactly this many trials -- quiet, contended, and the
/// unordered control. The other two subdivide it, because their work is a
/// product rather than a count:
///
/// | instrument | trials each | total |
/// |---|---|---|
/// | quiet / contended / control | `N` | `N` |
/// | concurrent rings | `(N / threads).max(5)` | that times `threads` |
/// | depth sweep | `(N / 4).max(5)` | that times 3 depths |
///
/// So at `N = 40` on an 8-thread machine: concurrent rings runs 5 per thread and
/// 40 in total, and the depth sweep runs 10 per depth and **30** in total.
/// Verified by running, not inferred from the arithmetic.
///
/// Two consequences worth knowing before reading a soak result or budgeting
/// disk writes:
///
/// - **The `.max(5)` floor dominates at small `N`.** Below 20, the depth sweep
///   runs 5 per depth whatever is asked, so `N = 4` and `N = 20` do the same
///   work there while the other instruments differ fivefold.
/// - **The depth sweep's divisor is 4 for 3 depths**, so it does roughly three
///   quarters of the budget rather than all of it. That is deliberate -- it is
///   the most expensive instrument per trial -- but it means the totals are not
///   uniform across a round.
///
/// **Zero is rejected rather than honoured.** A zero-trial run divides by zero
/// in [`Campaign::report`] and passes every assertion vacuously -- an instrument
/// that exercises no I/O and reports success is worse than one that fails, since
/// a soak tally full of passes is exactly what someone reads as evidence.
/// A value that does not parse is treated the same way: a typo should not
/// silently become the default.
fn trials() -> usize {
    match std::env::var("IORING_STRESS_TRIALS") {
        Err(_) => DEFAULT_TRIALS,
        Ok(raw) => match raw.trim().parse::<usize>() {
            Ok(0) => panic!(
                "IORING_STRESS_TRIALS=0 would run no trials, divide by zero when reporting, \
                 and pass every assertion without exercising anything. Use 1 or more, or \
                 unset it for the default of {DEFAULT_TRIALS}."
            ),
            Ok(n) => n,
            Err(error) => panic!(
                "IORING_STRESS_TRIALS={raw:?} is not a trial count ({error}). Use a positive \
                 integer, or unset it for the default of {DEFAULT_TRIALS}."
            ),
        },
    }
}

struct Aligned {
    storage: Vec<u8>,
    offset: usize,
    len: usize,
}

impl Aligned {
    fn new(len: usize, fill: u8) -> Self {
        let storage = vec![fill; len + ALIGN];
        let base = storage.as_ptr() as usize;
        let offset = (ALIGN - (base % ALIGN)) % ALIGN;
        Self {
            storage,
            offset,
            len,
        }
    }
}

// SAFETY: as in flush_barrier.rs -- `storage` is a heap allocation that is never
// reallocated while this value exists, so its address is stable across moves;
// `offset <= ALIGN` is within the over-allocation, and `len` bytes from there
// are initialized and stay allocated for the value's whole life.
unsafe impl IoBuf for Aligned {
    fn stable_ptr(&self) -> *const u8 {
        // SAFETY: `offset <= ALIGN` and the allocation is `len + ALIGN` bytes.
        unsafe { self.storage.as_ptr().add(self.offset) }
    }

    fn bytes_len(&self) -> usize {
        self.len
    }
}

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-flush-stress-{tag}-{}-{:?}.tmp",
        std::process::id(),
        std::thread::current().id()
    ))
}

fn open_unbuffered(path: &Path) -> OwnedHandle {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a null-terminated wide path that outlives the call; the
    // security-attributes and template arguments are the documented null
    // defaults.
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            std::ptr::null_mut(),
        )
    };
    assert!(
        !raw.is_null() && raw != INVALID_HANDLE_VALUE,
        "CreateFileW failed: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: `CreateFileW` just returned a fresh handle nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(raw.cast::<c_void>()) }
}

// ---------------------------------------------------------------------------
// The record kept for post-hoc diagnosis.
// ---------------------------------------------------------------------------

/// Which part of the submission an operation belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// Queued before the flush. The drain must complete after all of these (D-23).
    A,
    /// The flush itself.
    Flush,
    /// Queued after the flush. D-24 claimed these were held back; D-47 measured
    /// that they are not, so one completing early is documented, not a defect.
    B,
}

impl Phase {
    fn tag(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::Flush => "FLUSH",
            Self::B => "B",
        }
    }
}

/// One thing that happened, with enough context to reconstruct the trial.
#[derive(Clone, Debug)]
enum Event {
    /// An operation was pushed. `seq` is submission order.
    Submitted {
        seq: usize,
        user_data: usize,
        phase: Phase,
        offset: u64,
        len: usize,
    },
    /// `submit_and_wait` returned, with the count it reported.
    SubmittedAll { entries: u32, at: Duration },
    /// A drain round began -- one `try_pop` loop until it yields `None`.
    ///
    /// This is the field that separates "the CQ posted them in this order" from
    /// "we looked twice and the second look saw more": a mismatched pair inside
    /// one round was posted that way, a pair spanning rounds was not
    /// necessarily.
    RoundBegan { round: usize, at: Duration },
    /// A completion was popped.
    Popped {
        seq: usize,
        round: usize,
        user_data: usize,
        phase: Phase,
        bytes: usize,
        at: Duration,
    },
    /// A drain round ended, having yielded `drained` completions.
    RoundEnded { round: usize, drained: usize },
}

/// A bounded event log that keeps the most recent events.
///
/// A trial produces about 200 events, so the cap is never reached in practice
/// -- it is here so that a widened trial (more ops, more rounds) degrades by
/// dropping the oldest events and *saying so*, rather than by growing without
/// limit inside a stress loop.
struct EventLog {
    events: std::collections::VecDeque<Event>,
    cap: usize,
    dropped: usize,
    origin: Instant,
}

impl EventLog {
    fn new(cap: usize) -> Self {
        Self {
            events: std::collections::VecDeque::with_capacity(cap.min(4096)),
            cap,
            dropped: 0,
            origin: Instant::now(),
        }
    }

    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn push(&mut self, event: Event) {
        if self.events.len() == self.cap {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(event);
    }

    /// Render the log, with the operations that crossed the flush called out.
    fn render(&self, observed: &Observed) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();

        if self.dropped > 0 {
            let _ = writeln!(
                out,
                "  NOTE: {} earliest event(s) dropped -- the log reached its {}-event cap.",
                self.dropped, self.cap
            );
        }

        let _ = writeln!(
            out,
            "  flush user_data = {}, completion position {} of {}",
            observed.flush_id,
            observed.flush_position,
            observed.order.len()
        );
        let _ = writeln!(
            out,
            "  reordered_trials: {} phase-A after the flush, {} phase-B before it",
            observed.a_after_flush, observed.b_before_flush
        );
        if !observed.crossed.is_empty() {
            let _ = writeln!(
                out,
                "  crossed the flush, user_data: {:?}",
                observed.crossed
            );
            // The single most useful line in the dump. Inside one round, the
            // order is what the completion queue held when we looked, so the
            // kernel posted it that way. Across rounds, the two completions
            // were seen by separate looks, and the ordering claim is weaker.
            let _ = writeln!(
                out,
                "  the flush was popped in drain round {}; every crossing {} that round \
                 -- {}",
                observed.flush_round,
                if observed.all_crossings_in_flush_round {
                    "shared"
                } else {
                    "did NOT share"
                },
                if observed.all_crossings_in_flush_round {
                    "so this is the completion queue's own posting order"
                } else {
                    "so this spans separate looks at the queue, which is weaker evidence"
                }
            );
        }
        let _ = writeln!(out, "  --- events ---");

        for event in &self.events {
            match event {
                Event::Submitted {
                    seq,
                    user_data,
                    phase,
                    offset,
                    len,
                } => {
                    let _ = writeln!(
                        out,
                        "  submit #{seq:<3} ud={user_data:<5} {:<5} offset={offset:<12} len={len}",
                        phase.tag()
                    );
                }
                Event::SubmittedAll { entries, at } => {
                    let _ = writeln!(
                        out,
                        "  submit_and_wait returned entries={entries} at {:?}",
                        at
                    );
                }
                Event::RoundBegan { round, at } => {
                    let _ = writeln!(out, "  -- drain round {round} began at {at:?}");
                }
                Event::Popped {
                    seq,
                    round,
                    user_data,
                    phase,
                    bytes,
                    at,
                } => {
                    let mark = if observed.crossed.contains(user_data) {
                        " <<< CROSSED THE FLUSH"
                    } else if *phase == Phase::Flush {
                        " <<< the flush"
                    } else {
                        ""
                    };
                    let _ = writeln!(
                        out,
                        "  pop    #{seq:<3} ud={user_data:<5} {:<5} round={round:<3} bytes={bytes:<8} at={at:?}{mark}",
                        phase.tag()
                    );
                }
                Event::RoundEnded { round, drained } => {
                    let _ = writeln!(
                        out,
                        "  -- drain round {round} ended, {drained} completion(s)"
                    );
                }
            }
        }
        out
    }
}

/// What one trial observed.
struct Observed {
    a_after_flush: usize,
    b_before_flush: usize,
    /// Completion order, by user_data.
    order: Vec<usize>,
    flush_id: usize,
    flush_position: usize,
    /// The user_data of every operation on the wrong side of the flush.
    crossed: Vec<usize>,
    /// Which drain round the flush was popped in.
    flush_round: usize,
    /// Whether every crossing shared the flush's drain round.
    ///
    /// A crossing *within* one round is the CQ's own posting order. One that
    /// spans rounds means the two completions were seen by separate looks at
    /// the queue, which is a weaker claim about what the kernel did.
    all_crossings_in_flush_round: bool,
}

impl Observed {
    /// Did any completion cross the flush, in either direction?
    ///
    /// **Deliberately does not distinguish a failure from an expectation**, and
    /// so is never what an instrument asserts on the covering case. The two
    /// directions mean different things after
    /// [D-47](../DESIGN-NOTES.md#d-47-detail):
    ///
    /// - `a_after_flush` -- a write queued *before* the flush completing after
    ///   it. D-23, which holds, so this is a genuine contract failure.
    /// - `b_before_flush` -- a write queued *after* the flush completing before
    ///   it. The documented one-sided behaviour, not a defect.
    ///
    /// What this is for is the unordered control, where the question is only
    /// whether the queue reorders *at all* on this machine -- otherwise a
    /// passing covering result might mean nothing was ever there to reorder.
    fn reordered(&self) -> bool {
        self.a_after_flush > 0 || self.b_before_flush > 0
    }
}

/// Run one trial: phase A, one flush, phase B, submitted as a single batch.
fn run_trial(ring: &mut IoRing, file: RawHandle, coverage: FlushCoverage) -> (Observed, EventLog) {
    let mut log = EventLog::new(8192);
    let mut pending: HashMap<usize, Token<Aligned>> = HashMap::new();
    let mut phase_of: HashMap<usize, Phase> = HashMap::new();
    let mut expected_len: HashMap<usize, usize> = HashMap::new();
    let mut phase_a = Vec::with_capacity(PHASE_OPS);
    let mut phase_b = Vec::with_capacity(PHASE_OPS);
    let flush_id;
    let mut seq = 0;

    {
        let mut batch = Batch::new(ring);

        for index in 0..PHASE_OPS {
            let buffer = Aligned::new(BIG_LEN, index as u8);
            let offset = (index * BIG_LEN) as u64;
            // SAFETY: `file` stays open for the whole trial, and every token is
            // held in `pending` until its completion has been popped.
            let token = unsafe {
                batch.write_raw(
                    file,
                    buffer,
                    offset,
                    PushOptions::new(),
                    WriteCaching::Cached,
                )
            }
            .expect("queue phase-A write");
            let id = token.id();
            log.push(Event::Submitted {
                seq,
                user_data: id,
                phase: Phase::A,
                offset,
                len: BIG_LEN,
            });
            seq += 1;
            phase_a.push(id);
            phase_of.insert(id, Phase::A);
            expected_len.insert(id, BIG_LEN);
            pending.insert(id, token);
        }

        // SAFETY: as above.
        flush_id =
            unsafe { batch.flush_raw(file, coverage, FlushMode::Default) }.expect("queue flush");
        log.push(Event::Submitted {
            seq,
            user_data: flush_id,
            phase: Phase::Flush,
            offset: 0,
            len: 0,
        });
        seq += 1;
        phase_of.insert(flush_id, Phase::Flush);

        for index in 0..PHASE_OPS {
            let buffer = Aligned::new(SMALL_LEN, index as u8);
            let offset = (PHASE_B_BASE + index * SMALL_LEN) as u64;
            // SAFETY: as above.
            let token = unsafe {
                batch.write_raw(
                    file,
                    buffer,
                    offset,
                    PushOptions::new(),
                    WriteCaching::Cached,
                )
            }
            .expect("queue phase-B write");
            let id = token.id();
            log.push(Event::Submitted {
                seq,
                user_data: id,
                phase: Phase::B,
                offset,
                len: SMALL_LEN,
            });
            seq += 1;
            phase_b.push(id);
            phase_of.insert(id, Phase::B);
            expected_len.insert(id, SMALL_LEN);
            pending.insert(id, token);
        }

        let entries = batch
            .submit_and_wait((PHASE_OPS * 2 + 1) as u32, WAIT_MS)
            .expect("submit and wait for every completion");
        let at = log.now();
        log.push(Event::SubmittedAll { entries, at });
    }

    // The completion queue is FIFO, so pop order is completion order. The round
    // boundaries are recorded because "same round" and "different rounds" are
    // different strengths of evidence about what the kernel posted.
    let expected = PHASE_OPS * 2 + 1;
    let mut order = Vec::with_capacity(expected);
    let mut round = 0;
    let mut pop_seq = 0;
    let mut round_of: HashMap<usize, usize> = HashMap::new();
    let mut attempts = 0;

    while order.len() < expected {
        attempts += 1;
        assert!(
            attempts <= expected * 64,
            "only {} of {expected} completions ever arrived",
            order.len()
        );

        let at = log.now();
        log.push(Event::RoundBegan { round, at });
        let mut drained = 0;

        while let Some(completion) = ring.try_pop().expect("pop completion") {
            let transferred = completion.result().expect("write or flush succeeded");
            let id = completion.user_data();
            if let Some(&len) = expected_len.get(&id) {
                assert_eq!(
                    transferred, len,
                    "an unbuffered write transferred {transferred} of {len} bytes"
                );
            }
            if let Some(token) = pending.remove(&id) {
                let _buffer = token
                    .claim_if(&completion)
                    .expect("a token claims its own completion");
            }
            // Every submitted operation is recorded in `phase_of` -- phase A, the
            // flush, phase B -- so a miss here means the completion queue
            // returned a `user_data` this trial never submitted. That is a defect
            // worth stopping on, and the previous `unwrap_or(&Phase::Flush)` was
            // the worst possible response to it: an unknown completion would be
            // counted as a *second flush*, which is the one identity the whole
            // analysis pivots on. The result would be a confusing measurement
            // rather than a clear failure, in a harness whose entire purpose is
            // making a rare reordering diagnosable.
            let phase = *phase_of.get(&id).unwrap_or_else(|| {
                let mut submitted: Vec<usize> = phase_of.keys().copied().collect();
                submitted.sort_unstable();
                panic!(
                    "completion carried user_data {id}, which this trial never submitted. \
                     The flush is {flush_id}; the {} submitted ids are {submitted:?}",
                    submitted.len(),
                )
            });
            log.push(Event::Popped {
                seq: pop_seq,
                round,
                user_data: id,
                phase,
                bytes: transferred,
                at: log.now(),
            });
            pop_seq += 1;
            round_of.insert(id, round);
            order.push(id);
            drained += 1;
        }

        log.push(Event::RoundEnded { round, drained });
        round += 1;

        // Yield when a round came back empty, rather than spinning the core.
        //
        // **Deliberately `yield_now` and not a sleep**, which matters more here
        // than it usually would. Drain-round boundaries are evidence: D-47 rests
        // partly on every crossing being confined to the flush's own round, which
        // is what shows the order observed is the order the kernel posted. A
        // fixed delay would let completions accumulate between polls, coarsening
        // those rounds and making "same round" artificially more likely -- the
        // instrument would then be manufacturing its own conclusion.
        //
        // Yielding relinquishes the timeslice without introducing a delay, so
        // other runnable threads -- the contention generators, and the other
        // rings in the concurrent instrument -- make progress instead of
        // competing with a spin. The `attempts` bound above still limits the
        // total.
        if drained == 0 {
            std::thread::yield_now();
        }
    }

    let flush_position = order
        .iter()
        .position(|&id| id == flush_id)
        .expect("the flush's own completion");
    let flush_round = *round_of.get(&flush_id).expect("the flush was popped");

    let mut crossed = Vec::new();
    let a_after_flush = order[flush_position + 1..]
        .iter()
        .filter(|id| phase_a.contains(id))
        .inspect(|id| crossed.push(**id))
        .count();
    let b_before_flush = order[..flush_position]
        .iter()
        .filter(|id| phase_b.contains(id))
        .inspect(|id| crossed.push(**id))
        .count();

    let all_crossings_in_flush_round = crossed
        .iter()
        .all(|id| round_of.get(id) == Some(&flush_round));

    (
        Observed {
            a_after_flush,
            b_before_flush,
            order,
            flush_id,
            flush_position,
            crossed,
            flush_round,
            all_crossings_in_flush_round,
        },
        log,
    )
}

/// Background disk load, to make contention a variable rather than an accident.
struct Contention {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Contention {
    /// Spawn `threads` writers, each hammering its own unbuffered file.
    fn start(threads: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let handles = (0..threads)
            .map(|n| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    // Named through the same helper the fixtures use, which adds
                    // the thread id. Without it the path was only pid + writer
                    // index, so every `Contention::start(4)` in the process
                    // produced the *same four paths* -- and three instruments
                    // start one each. Run with the default parallel harness, as
                    // this file's own documented command did, they overwrote each
                    // other's files and deleted them on the way out, so each
                    // instrument's load was not the load it thought it had.
                    // Measured before the fix: four load files where twelve were
                    // expected.
                    //
                    // This runs *on the writer thread*, so the id is that
                    // writer's own and is unique across the process -- Rust
                    // guarantees a `ThreadId` is never reused.
                    let path = temp_file(&format!("load-{n}"));
                    let payload = vec![0xA5_u8; 4 * 1024 * 1024];
                    while !stop.load(Ordering::Relaxed) {
                        // Ordinary buffered writes plus a flush: the point is to
                        // occupy the device and the filesystem, not to be
                        // elegant about it.
                        if let Ok(()) = std::fs::write(&path, &payload) {
                            let _ = std::fs::File::open(&path).map(|f| f.sync_all());
                        }
                    }
                    let _ = std::fs::remove_file(&path);
                })
            })
            .collect();
        // Let the load actually start before the measurement does.
        std::thread::sleep(Duration::from_millis(250));
        Self {
            stop,
            threads: handles,
        }
    }
}

impl Drop for Contention {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

/// One fixture: a pre-written extent, opened unbuffered, plus a ring.
///
/// The handle is an `Option` so [`Drop`] can close it *before* deleting the
/// path. Struct fields drop after the `Drop` body runs, so holding it directly
/// meant `remove_file` was called while the handle was still open -- and the
/// file is opened with `FILE_SHARE_READ | FILE_SHARE_WRITE` and no
/// `FILE_SHARE_DELETE`, so that deletion cannot succeed. Every trial leaked its
/// 32 MiB extent, silently, because the result was discarded.
struct Fixture {
    file: Option<OwnedHandle>,
    handle: RawHandle,
    ring: IoRing,
    path: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let extent = PHASE_B_BASE + PHASE_OPS * SMALL_LEN;
        let path = temp_file(tag);
        std::fs::write(&path, vec![0_u8; extent]).expect("pre-write the extent");
        let file = open_unbuffered(&path);
        let handle = file.as_raw_handle();
        let ring = IoRing::new(256, 256).expect("create ring");
        Self {
            file: Some(file),
            handle,
            ring,
            path,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Close first; see the note on the struct. `handle` is dangling from here
        // on, and nothing below touches it.
        drop(self.file.take());

        // Reported rather than discarded. A silent `let _ =` is what let the leak
        // above run for a whole session unnoticed, and a warning during teardown
        // costs nothing while a lost 32 MiB per trial does not stay small.
        if let Err(error) = std::fs::remove_file(&self.path) {
            eprintln!(
                "warning: could not remove the stress fixture {}: {error}",
                self.path.display()
            );
        }
    }
}

/// Outcome of a repeated run.
///
/// The two directions are counted **separately and on purpose**, because after
/// [D-47](../DESIGN-NOTES.md#d-47-detail) they are no longer the same kind of
/// event:
///
/// - **D-23** -- the flush waits for what precedes it. This holds, so a phase-A
///   write completing after the flush is a genuine **failure** of a live
///   contract, and is what the instruments assert.
/// - **D-24's withdrawn half** -- that operations queued after are held back.
///   They are not, so a phase-B write completing early is the **documented
///   one-sided behaviour**, not a defect. It is counted to measure its rate, and
///   never asserted against.
///
/// A single combined counter would collapse a failure and an expectation into
/// one number, which is exactly the distinction this campaign exists to draw.
struct Campaign {
    trials: usize,
    /// Trials where any completion crossed the flush, in either direction.
    ///
    /// Deliberately *not* the headline number: it mixes the two above. It is
    /// asserted only by the unordered control, where the question is whether
    /// reordering happens at all without a barrier.
    reordered_trials: usize,
    /// Trials where a phase-A write completed after the flush. **D-23's failure**
    /// -- a broken contract.
    d23_failures: usize,
    /// Trials where a phase-B write completed before the flush. The **documented
    /// one-sided behaviour** (D-47), reported rather than asserted.
    d24_overtakes: usize,
    /// Total phase-B writes seen ahead of the flush, across all trials.
    d24_total_overtakers: usize,
    /// The largest number of overtakers in any one trial.
    d24_worst: usize,
    /// The first reordered trial's rendered log, if any.
    first_reordered: Option<String>,
    /// A clean trial's rendered log, for comparison.
    first_clean: Option<String>,
    /// Reordered trials whose crossings all shared the flush's drain round.
    same_round: usize,
}

impl Campaign {
    fn new(trials: usize) -> Self {
        Self {
            trials,
            reordered_trials: 0,
            d23_failures: 0,
            d24_overtakes: 0,
            d24_total_overtakers: 0,
            d24_worst: 0,
            first_reordered: None,
            first_clean: None,
            same_round: 0,
        }
    }

    fn absorb(&mut self, observed: &Observed) {
        if observed.a_after_flush > 0 {
            self.d23_failures += 1;
        }
        if observed.b_before_flush > 0 {
            self.d24_overtakes += 1;
            self.d24_total_overtakers += observed.b_before_flush;
            self.d24_worst = self.d24_worst.max(observed.b_before_flush);
        }
        if observed.reordered() {
            self.reordered_trials += 1;
            if observed.all_crossings_in_flush_round {
                self.same_round += 1;
            }
        }
    }

    fn report(&self, what: &str) -> String {
        format!(
            "{what}: {} of {} trial(s) had a completion cross the flush ({:.2}%) -- D-23 \
             (the flush waits for preceding writes) FAILED {} time(s); D-24's withdrawn \
             hold-back claim: {} trial(s) where a later write completed first (documented \
             one-sided behaviour; not a defect); {} overtaking write(s) in total; worst {} in \
             one trial; {} crossing(s) confined to the flush's own drain round",
            self.reordered_trials,
            self.trials,
            100.0 * self.reordered_trials as f64 / self.trials as f64,
            self.d23_failures,
            self.d24_overtakes,
            self.d24_total_overtakers,
            self.d24_worst,
            self.same_round
        )
    }
}

/// Run `count` trials with `coverage`, keeping the first reordered log and the
/// first clean one.
fn campaign(tag: &str, coverage: FlushCoverage, count: usize) -> Campaign {
    let mut fixture = Fixture::new(tag);
    let mut result = Campaign::new(count);

    for trial in 0..count {
        let (observed, log) = run_trial(&mut fixture.ring, fixture.handle, coverage);
        result.absorb(&observed);
        if observed.reordered() {
            if result.first_reordered.is_none() {
                result.first_reordered = Some(format!(
                    "  trial {trial} of {count}, coverage {coverage:?}\n{}",
                    log.render(&observed)
                ));
            }
        } else if result.first_clean.is_none() {
            result.first_clean = Some(format!(
                "  trial {trial} of {count}, coverage {coverage:?}\n{}",
                log.render(&observed)
            ));
        }
    }

    result
}

// ---------------------------------------------------------------------------
// The instruments.
// ---------------------------------------------------------------------------

/// Does the drain hold, and how often is the one-sidedness visible, on an idle disk?
///
/// This is the baseline the contended run is compared against. A violation here
/// would mean contention is not the trigger at all, which would point at the
/// barrier's guarantee rather than at load.
#[test]
#[ignore = "writes tens of MiB per trial; run explicitly"]
fn the_drain_holds_across_many_quiet_trials() {
    let count = trials();
    let result = campaign("quiet", FlushCoverage::CoversPrecedingOperations, count);

    eprintln!("{}", result.report("quiet"));
    if let Some(clean) = &result.first_clean {
        eprintln!("A CLEAN TRIAL, for comparison:\n{clean}");
    }
    if let Some(reordered) = &result.first_reordered {
        eprintln!("THE FIRST TRIAL WHERE A COMPLETION CROSSED THE FLUSH:\n{reordered}");
    }

    // Only D-23 is asserted. A phase-B write completing early is the documented
    // one-sided behaviour (D-47), not a failure -- this very instrument is what
    // established it, at about 0.03% on an idle ring, so asserting it to be zero
    // would make the test fail on the platform behaving as documented.
    assert_eq!(
        result.d23_failures,
        0,
        "{}",
        result.report("a write queued BEFORE the flush completed after it, with no competing load")
    );
}

/// Does the drain hold while the disk is deliberately saturated?
///
/// This is the one built to reproduce the observed failure. It is expected to
/// be the test that fails; when it does, the log it prints is the evidence the
/// two readings in the module docs have to be decided on.
#[test]
#[ignore = "saturates a disk with background writers; run explicitly"]
fn the_drain_holds_under_deliberate_disk_contention() {
    let count = trials();
    let _load = Contention::start(4);
    let result = campaign("contended", FlushCoverage::CoversPrecedingOperations, count);

    eprintln!("{}", result.report("contended"));
    if let Some(clean) = &result.first_clean {
        eprintln!("A CLEAN TRIAL, for comparison:\n{clean}");
    }
    if let Some(reordered) = &result.first_reordered {
        eprintln!("THE FIRST TRIAL WHERE A COMPLETION CROSSED THE FLUSH:\n{reordered}");
    }

    // D-23 only; see the quiet instrument for why D-24 is reported, not asserted.
    assert_eq!(
        result.d23_failures,
        0,
        "{}",
        result.report("a write queued BEFORE the flush completed after it, under contention")
    );
}

/// Under the same contention, can the control still tell a barrier from none?
///
/// This guards the whole comparison rather than the contract. If load makes an
/// *unordered* flush stop reordering -- or makes everything reorder -- then the
/// contended covering result means nothing either way, and that is a finding
/// about the instrument rather than about the kernel.
///
/// It asserts nothing about the barrier on purpose. It reports, and fails only
/// if the control has lost its ability to discriminate.
#[test]
#[ignore = "saturates a disk with background writers; run explicitly"]
fn the_unordered_control_still_discriminates_under_contention() {
    let count = trials();
    let _load = Contention::start(4);
    let result = campaign("control", FlushCoverage::Unordered, count);

    eprintln!("{}", result.report("unordered control under contention"));
    if let Some(sample) = &result.first_reordered {
        eprintln!("A REORDERED CONTROL TRIAL:\n{sample}");
    }

    assert!(
        result.reordered_trials > 0,
        "an unordered flush produced completions in strict submission order in all {count} \
         contended trials. The covering assertion would then pass for the wrong reason under \
         contention, so a contended covering result proves nothing until this is understood.",
    );
}

/// Does the drain hold when several rings are working the device at once?
///
/// This is a **different and probably better** pressure than
/// [`the_drain_holds_under_deliberate_disk_contention`], and worth
/// having as well rather than instead.
///
/// That test loads the *device*, with background threads doing ordinary
/// buffered writes. This one loads the **IoRing subsystem itself**: every
/// competing thread is submitting to its own ring, waiting, and draining, so
/// whatever kernel machinery implements `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` is
/// under contention rather than merely the disk beneath it. If the drain has a
/// defect that contention exposes, this is the shape most likely to find it,
/// because it competes with the drain rather than around it.
///
/// **Each thread gets its own ring and its own file, deliberately.** The drain's
/// *reach* is ring-wide -- that half of D-24 survived D-47, which withdrew only
/// the hold-back claim -- so a per-thread ring keeps each assertion a
/// statement about its own ring and nothing else; and separate files keep the
/// filesystem's own write serialisation -- one of the three failure modes the
/// contract test had to design around -- out of the result. Sharing either
/// would produce a livelier test whose failures could not be attributed.
///
/// No background load is added on top: here the other rings *are* the load, and
/// stacking both would confound the two variables this pair exists to separate.
#[test]
#[ignore = "saturates a disk from several rings at once; run explicitly"]
fn the_drain_holds_with_concurrent_rings() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 8))
        .unwrap_or(4);
    let count = (trials() / threads).max(5);

    eprintln!("Concurrent rings: {threads} thread(s), {count} trial(s) each.");

    let workers: Vec<_> = (0..threads)
        .map(|worker| {
            std::thread::spawn(move || {
                // Built inside the thread so each ring and handle belongs to
                // the thread that uses it.
                campaign(
                    &format!("concurrent-{worker}"),
                    FlushCoverage::CoversPrecedingOperations,
                    count,
                )
            })
        })
        .collect();

    let mut total = Campaign::new(0);
    let mut first_reordered = None;
    let mut first_clean = None;

    // Collected rather than printed here, so the TOTAL can be emitted first; see
    // the note below the loop.
    let mut per_worker = Vec::with_capacity(threads);

    for (worker, handle) in workers.into_iter().enumerate() {
        let result = handle.join().expect("a worker thread panicked");
        per_worker.push(result.report(&format!("worker {worker}")));
        total.trials += result.trials;
        total.reordered_trials += result.reordered_trials;
        total.d23_failures += result.d23_failures;
        total.d24_overtakes += result.d24_overtakes;
        total.d24_total_overtakers += result.d24_total_overtakers;
        total.d24_worst = total.d24_worst.max(result.d24_worst);
        total.same_round += result.same_round;
        if first_reordered.is_none() {
            first_reordered = result.first_reordered;
        }
        if first_clean.is_none() {
            first_clean = result.first_clean;
        }
    }

    // The TOTAL first, then the per-worker breakdown.
    //
    // Ordering matters because it is scraped, not just read:
    // `tools/soak-flush-barrier.ps1` takes the *first* line matching `D-23` as
    // the CSV's detail column. With the per-worker lines printed as each thread
    // was joined, the CSV recorded worker 0 -- one thread of `threads` -- while
    // being labelled with the instrument's name, which is a quietly wrong number
    // rather than a missing one.
    //
    // The total was also not printed at all on a passing run: it existed only
    // inside the assertion message below, which renders on failure. So the
    // figure a reader most wants was the one they could never get.
    eprintln!(
        "{}",
        total.report(&format!("{threads} concurrent rings TOTAL"))
    );
    for line in &per_worker {
        eprintln!("  {line}");
    }

    if let Some(clean) = &first_clean {
        eprintln!("A CLEAN TRIAL, for comparison:\n{clean}");
    }
    if let Some(reordered) = &first_reordered {
        eprintln!("THE FIRST TRIAL WHERE A COMPLETION CROSSED THE FLUSH:\n{reordered}");
    }

    // D-23 only, as in the single-ring instruments. D-24 failures are expected
    // here -- this is the condition that produced the highest rate of them.
    assert_eq!(
        total.d23_failures,
        0,
        "{}",
        total.report(&format!(
            "a write queued BEFORE the flush completed after it, across {threads} concurrent rings"
        ))
    );
}

/// Does the overtake rate track queue depth?
///
/// Reports rather than asserts. If reordered_trials appear only at depth, that is a
/// different shape of answer from a fixed per-trial probability, and it is
/// cheap to find out while the instrument is already built.
#[test]
#[ignore = "writes tens of MiB per trial; run explicitly"]
fn reordering_rate_by_ring_depth_is_reported() {
    let count = (trials() / 4).max(5);
    let _load = Contention::start(4);

    // A trial submits PHASE_OPS writes, one flush, and PHASE_OPS more as a
    // single batch, so the submission queue must hold all 65 at once -- the
    // contract test says so, and a depth of 64 does not: it fails with
    // IORING_E_SUBMISSION_QUEUE_FULL (0x80460002) rather than measuring
    // anything. Found by running this sweep, which originally started at 64.
    //
    // So what varies here is the ring's *headroom* over a fixed shape, not the
    // shape itself. That is still the interesting axis: if reordered_trials need
    // slack in the queue, the rate should move across these.
    const PER_TRIAL_OPS: u32 = (PHASE_OPS * 2 + 1) as u32;

    // Collected before anything is printed, for the same scraping reason as the
    // concurrent-rings instrument: the soak runner takes the *first* `D-23` line
    // as the CSV's detail, so the first line an instrument prints must be the one
    // that represents it. Printing per-depth lines as they were produced put
    // `depth 128` in a column labelled with the whole sweep's name.
    let mut per_depth = Vec::new();
    let mut combined = Campaign::new(0);

    for depth in [128_u32, 256, 512] {
        assert!(
            depth >= PER_TRIAL_OPS,
            "a depth of {depth} cannot hold one trial's {PER_TRIAL_OPS} entries"
        );
        let mut fixture = Fixture::new(&format!("depth-{depth}"));
        fixture.ring = IoRing::new(depth, depth).expect("create ring");
        let mut result = Campaign::new(count);
        for _ in 0..count {
            let (observed, _log) = run_trial(
                &mut fixture.ring,
                fixture.handle,
                FlushCoverage::CoversPrecedingOperations,
            );
            result.absorb(&observed);
        }

        combined.trials += result.trials;
        combined.reordered_trials += result.reordered_trials;
        combined.d23_failures += result.d23_failures;
        combined.d24_overtakes += result.d24_overtakes;
        combined.d24_total_overtakers += result.d24_total_overtakers;
        combined.d24_worst = combined.d24_worst.max(result.d24_worst);
        combined.same_round += result.same_round;

        per_depth.push(result.report(&format!("depth {depth}")));
    }

    // The combined figure is the headline because it is the one that is
    // comparable **across soak rounds** -- which is what the CSV is for. It is
    // explicitly not the instrument's finding: pooling the depths averages away
    // the very axis this measures, so the per-depth breakdown follows
    // immediately and is what a reader should actually compare.
    eprintln!(
        "Ring depth sweep, {count} trial(s) each, under contention -- {}",
        combined.report("ALL DEPTHS COMBINED")
    );
    for line in &per_depth {
        eprintln!("  {line}");
    }
}
