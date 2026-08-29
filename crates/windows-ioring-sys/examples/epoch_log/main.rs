// Copyright (c) 2026 Mike Grier
//! `epoch-log` (M13): a miniature write-ahead log on `IoRing`, made durable by
//! group commit and reporting durability by epoch.
//!
//! This is a **sample**, not library surface. `windows-ioring-sys` exposes
//! Windows mechanism and deliberately owns no durability *policy* (D-26 in its
//! `DESIGN-NOTES.md`): epoch bookkeeping, which barrier strategy to pay for,
//! and how durability is reported all depend on a consumer's workload and
//! contract, so baking them into a primitive would be exactly the kind of
//! policy D-8 refuses. The residue is that every consumer would otherwise
//! rediscover the same composition, and the contracts involved (D-19's
//! edge-triggered completion event, D-23's non-covering flush, D-24's
//! ring-wide barrier) are the kind that get learned by deadlock or by data
//! loss. This sample is the transfer vehicle.
//!
//! # Read [`contract`] first
//!
//! The sample's own durability contract is written down in [`contract`], and
//! it was written *before* any of the code that implements it. That ordering
//! is deliberate: the contract is this program's specification, phrased in its
//! own words rather than as a description of what the ring happens to do. The
//! code below has to satisfy it; it does not get to define it.
//!
//! # State of construction
//!
//! M13.1 delivered the contract, M13.2 the record format and append path,
//! M13.3 epoch bookkeeping and group commit; M13.4 (this) replaces the fused
//! wait with the multiplexed one, so the log can also wake for something that
//! is not ring I/O. What finishes it:
//!
//! - **M13.5** -- replay and verify, which is the only part that can actually
//!   catch a durability bug.

mod append;
mod commit;
mod contract;
mod event_loop;
mod record;
mod replay;

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::mpsc;

use append::{Appender, SLOT_LEN, SLOTS};
use commit::{Committer, Epoch};
use contract::{CONTRACT, Clause};
use event_loop::{EventLoop, Woken};
use windows_ioring_sys::{Batch, IoRing};

/// How many records this demonstration appends into committed epochs.
const RECORDS: usize = 24;

/// How many it then appends into an epoch it never commits, so replay has a
/// tail to tolerate.
const TAIL_RECORDS: usize = 3;

/// Records per epoch. Small so the demonstration shows several commits; a real
/// log sizes this against its latency target, since the commit is what costs.
const EPOCH_SIZE: usize = 6;

/// Bound on the wait for a commit, so a stuck example fails instead of hanging.
const WAIT_MS: u32 = 30_000;

/// Bound on the shutdown quiesce, for the same reason.
const QUIESCE_ATTEMPTS: usize = 64;

/// The single sink every line of this sample's output goes through (the
/// repository's "architectural pre-steps" rule: never call `println!` from
/// more than one site -- introduce the abstraction at the first occurrence).
/// `out` carries ordinary output, `err` carries failures; both are plain
/// `Write` streams, so a caller could redirect either without touching a call
/// site.
struct Report<O, E> {
    out: O,
    err: E,
}

impl<O: io::Write, E: io::Write> Report<O, E> {
    fn new(out: O, err: E) -> Self {
        Self { out, err }
    }

    fn line(&mut self, args: std::fmt::Arguments<'_>) {
        let _ = writeln!(self.out, "{args}");
    }

    fn error_line(&mut self, args: std::fmt::Arguments<'_>) {
        let _ = writeln!(self.err, "{args}");
    }
}

/// Where this run's log file lives. Removed on the way out; M13.5's replay
/// pass is what will read it back before that happens.
fn log_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-log-{}.log",
        std::process::id()
    ))
}

/// The payload of the `index`-th record this demonstration appends. One
/// function so the append side and the read-back check cannot drift.
fn payload_for(index: usize) -> Vec<u8> {
    format!("record {index}: the quick brown fox").into_bytes()
}

/// What one run of the log reported about itself, for replay to check.
struct LogRun {
    durable_through: Epoch,
    durable_records: usize,
    tail_records: usize,
}

/// Append `RECORDS` records in epochs of `EPOCH_SIZE`, committing each one and
/// waiting for it to become durable, then append `TAIL_RECORDS` more into an
/// epoch that is deliberately **never committed**.
///
/// That uncommitted tail is not padding: the contract promises nothing about
/// records past the last committed epoch, and a replay pass that never sees
/// such a tail has not been asked the interesting question.
fn run_log<O: io::Write, E: io::Write>(
    report: &mut Report<O, E>,
    path: &std::path::Path,
) -> io::Result<LogRun> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(64, 128)?;
    let mut appender = Appender::new(&mut ring)?;
    let mut committer = Committer::new();
    let events = EventLoop::new(&mut ring)?;
    report.line(format_args!(
        "arena registered: {SLOTS} slots of {SLOT_LEN} bytes; \
         waiting on the ring's completion event alongside a shutdown latch"
    ));

    // Something outside the I/O loop decides when to stop -- which is the only
    // reason a second handle is in the wait at all.
    let shutdown = events.shutdown_handle()?;
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let control = std::thread::spawn(move || {
        if stop_rx.recv().is_ok() {
            let _ = event_loop::signal(&shutdown);
        }
    });

    let mut appended = 0;
    let mut empty_wakes = 0;
    while appended < RECORDS {
        let epoch = committer.open_epoch();
        let payload = payload_for(appended);
        match appender.append(&mut ring, handle, epoch, &payload) {
            Ok(_sequence) => appended += 1,
            // Every slot is in flight. This is the arena working as intended,
            // not an error: pump once to drain and try again.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let (_, popped) =
                    events.pump(WAIT_MS, || drain(&mut ring, &mut appender, &mut committer))?;
                empty_wakes += usize::from(popped == 0);
            }
            Err(error) => return Err(error),
        }

        if appended % EPOCH_SIZE == 0 {
            let closed = committer.commit(&mut ring, handle)?;
            // Before the commit's completion is observed, the honest answer is
            // "no". Asking here rather than after is what makes the difference
            // between the two visible.
            assert!(
                !committer.is_durable(closed),
                "a pushed commit is not a completed one"
            );
            while !committer.is_durable(closed) {
                let (_, popped) =
                    events.pump(WAIT_MS, || drain(&mut ring, &mut appender, &mut committer))?;
                empty_wakes += usize::from(popped == 0);
            }
            report.line(format_args!(
                "epoch {} committed and durable: {appended} records so far, \
                 durable through epoch {}",
                closed.0,
                committer
                    .durable_through()
                    .map_or_else(|| "none".to_owned(), |e| e.0.to_string()),
            ));
        }
    }

    // The uncommitted tail: appended, so their writes complete, but no commit
    // ever closes their epoch. The contract therefore promises nothing about
    // them, and the replay pass below is what proves the reader tolerates that.
    for index in RECORDS..RECORDS + TAIL_RECORDS {
        let epoch = committer.open_epoch();
        let payload = payload_for(index);
        loop {
            match appender.append(&mut ring, handle, epoch, &payload) {
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    events.pump(WAIT_MS, || drain(&mut ring, &mut appender, &mut committer))?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    report.line(format_args!(
        "appended {TAIL_RECORDS} more records into epoch {} and deliberately never committed it",
        committer.open_epoch().0
    ));

    // The work is done: ask for shutdown and keep pumping until the *other*
    // handle is what wakes us. The drain still runs on that pass -- see
    // `EventLoop::pump` -- which is the rule this loop exists to demonstrate.
    let _ = stop_tx.send(());
    loop {
        let (woken, popped) =
            events.pump(WAIT_MS, || drain(&mut ring, &mut appender, &mut committer))?;
        empty_wakes += usize::from(popped == 0);
        if woken == Woken::Shutdown {
            report.line(format_args!(
                "shutdown woke the wait with {} append(s) and {} commit(s) still in flight",
                appender.in_flight(),
                committer.in_flight()
            ));
            break;
        }
    }
    drop(stop_tx);
    control.join().expect("control thread");

    // Shutdown with I/O in flight is normal, not an error: the kernel may
    // still be reading arena slots, so nothing may close until they finish.
    // Every SQE that queued produces exactly one completion, so this ends.
    let mut attempts = 0;
    while appender.in_flight() > 0 || committer.in_flight() > 0 {
        attempts += 1;
        if attempts > QUIESCE_ATTEMPTS {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "outstanding operations never completed during shutdown quiesce",
            ));
        }
        Batch::new(&mut ring).submit_and_wait(1, WAIT_MS)?;
        drain(&mut ring, &mut appender, &mut committer)?;
    }

    let durable_through = committer
        .durable_through()
        .expect("at least one epoch was committed");
    report.line(format_args!(
        "appended {appended} records across {} committed epochs plus {TAIL_RECORDS} uncommitted \
         (sequences 0..{}); durable through epoch {}",
        durable_through.0 + 1,
        appender.next_sequence().0,
        durable_through.0
    ));
    report.line(format_args!(
        "{empty_wakes} wake(s) had nothing to pop, which the contract requires a caller to tolerate"
    ));
    // Monotonicity is a guarantee, so it is checked rather than assumed.
    for epoch in 0..=durable_through.0 {
        assert!(
            committer.is_durable(Epoch(epoch)),
            "durability must be monotonic: epoch {epoch} is below the durable watermark"
        );
    }
    assert!(
        !committer.is_durable(committer.open_epoch()),
        "the still-open epoch was never committed and must not report durable"
    );

    drop(file);
    Ok(LogRun {
        durable_through,
        durable_records: RECORDS,
        tail_records: TAIL_RECORDS,
    })
}

/// Replay the log three ways, which is what turns this sample from a
/// demonstration into evidence (M13.5).
fn verify<O: io::Write, E: io::Write>(
    report: &mut Report<O, E>,
    path: &std::path::Path,
    run: &LogRun,
) -> io::Result<()> {
    let bytes = std::fs::read(path)?;

    // 1. The log as written. Everything committed must be intact, and the
    //    uncommitted tail must be tolerated rather than rejected.
    let clean = replay::replay(
        &bytes,
        run.durable_through,
        run.durable_records,
        payload_for,
    );
    report.line(format_args!(
        "replay: {} durable records verified, {} uncommitted tail record(s) tolerated{}",
        clean.durable_verified,
        clean.tail_records,
        clean
            .tail_stopped
            .map_or_else(String::new, |reason| format!(", stopped at {reason:?}"))
    ));
    assert!(
        clean.is_clean(),
        "the log must honour its own contract: {:?}",
        clean.violations
    );
    assert_eq!(clean.durable_verified, run.durable_records);
    assert_eq!(clean.tail_records, run.tail_records);

    // 2. A torn tail, which is what a crash actually leaves behind. Cutting
    //    the file mid-record simulates a write that did not land whole. The
    //    contract says the reader must tolerate this, so a violation here
    //    would mean the reader is stricter than the contract allows.
    let torn_at = bytes.len() - (record::HEADER_LEN + 4);
    let torn = replay::replay(
        &bytes[..torn_at],
        run.durable_through,
        run.durable_records,
        payload_for,
    );
    report.line(format_args!(
        "replay of a torn tail: {} durable records still verified, {} tail record(s), \
         stopped at {:?}",
        torn.durable_verified, torn.tail_records, torn.tail_stopped
    ));
    assert!(
        torn.is_clean(),
        "a torn tail is a legal outcome and must not be reported as a violation: {:?}",
        torn.violations
    );
    assert_eq!(
        torn.durable_verified, run.durable_records,
        "tearing the tail must not cost a single durable record"
    );

    // 3. The negative control. A verifier that cannot fail proves nothing, so
    //    corrupt one byte *inside* the durable region and require that replay
    //    notices. If this passes, the two checks above mean something.
    let mut damaged = bytes.clone();
    let victim = record::HEADER_LEN + 2;
    damaged[victim] ^= 0xFF;
    let caught = replay::replay(
        &damaged,
        run.durable_through,
        run.durable_records,
        payload_for,
    );
    report.line(format_args!(
        "negative control: corrupting byte {victim} of a durable record was caught as {:?}",
        caught.violations
    ));
    assert!(
        !caught.is_clean(),
        "corruption inside the durable region must be reported, or this verifier checks nothing"
    );

    Ok(())
}

/// Block until `epoch` is durable, draining completions while we wait.
///
/// Kept for reference: this is the *fused* wait, which is what M13.3 used
/// before the multiplexed loop arrived. It is correct for a log whose only
/// I/O is ring I/O, and cheaper -- there is nothing to re-arm and no
/// edge-trigger rule to obey. It cannot serve a log that must also wake for a
/// shutdown latch, which is why `run_log` above uses `EventLoop::pump`
/// instead. Both are Model B; only what the thread blocks on differs.
#[expect(
    dead_code,
    reason = "kept as the documented contrast with the multiplexed wait; M14.3 makes the \
              choice between wakeup strategies selectable at run time"
)]
fn await_durable_fused(
    ring: &mut IoRing,
    appender: &mut Appender,
    committer: &mut Committer,
    epoch: Epoch,
) -> io::Result<()> {
    while !committer.is_durable(epoch) {
        if drain(ring, appender, committer)? == 0 {
            Batch::new(ring).submit_and_wait(1, WAIT_MS)?;
        }
    }
    Ok(())
}

/// Pop every completion currently available, routing each to whichever of the
/// two owns it.
fn drain(
    ring: &mut IoRing,
    appender: &mut Appender,
    committer: &mut Committer,
) -> io::Result<usize> {
    let mut popped = 0;
    while let Some(completion) = ring.try_pop()? {
        // A completion belongs to exactly one of the two, so the short-circuit
        // is the dispatch: if the appender claimed it the committer is never
        // asked, and a completion neither recognises is left uncounted rather
        // than silently attributed.
        if appender.claim(&completion)? || committer.claim(&completion)?.is_some() {
            popped += 1;
        }
    }
    Ok(popped)
}

/// Print the contract the rest of this sample has to satisfy.
///
/// A reader who only ever runs the sample still learns what it does and does
/// not promise, which for a program whose entire subject is durability is the
/// most important thing it can say.
fn report_contract<O: io::Write, E: io::Write>(report: &mut Report<O, E>) {
    report.line(format_args!("epoch-log durability contract"));
    report.line(format_args!("=============================="));

    for clause in [
        Clause::Guarantees,
        Clause::DoesNotGuarantee,
        Clause::Assumes,
    ] {
        report.line(format_args!(""));
        report.line(format_args!("This log {}:", clause.heading()));
        for statement in CONTRACT.iter().filter(|s| s.clause == clause) {
            report.line(format_args!("  - {}", statement.text));
        }
    }
}

fn main() {
    let mut report = Report::new(io::stdout(), io::stderr());
    report_contract(&mut report);
    report.line(format_args!(""));

    let path = log_path();
    let outcome = run_log(&mut report, &path).and_then(|run| {
        report.line(format_args!(""));
        verify(&mut report, &path, &run)
    });
    match outcome {
        Ok(()) => report.line(format_args!(
            "the log kept its contract, and the verifier that says so was shown to be able to fail"
        )),
        Err(error) => {
            report.error_line(format_args!("epoch log failed: {error}"));
            let _ = std::fs::remove_file(&path);
            std::process::exit(1);
        }
    }

    let _ = std::fs::remove_file(&path);
}
