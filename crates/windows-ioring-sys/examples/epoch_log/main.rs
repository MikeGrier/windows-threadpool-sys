// Copyright (c) 2026 Mike Grier
//! `epoch-log`: a miniature write-ahead log on `IoRing`, made durable by group
//! commit and reporting durability by epoch.
//!
//! # This is a demonstration, not API
//!
//! Nothing here is public surface, nothing here is covered by this crate's
//! semantic versioning, and copying it makes its policy choices yours to
//! maintain. That is not a disclaimer bolted on; it is the reason the sample
//! exists at all.
//!
//! `windows-ioring-sys` exposes Windows mechanism and deliberately owns no
//! durability *policy* (D-26 in its `DESIGN-NOTES.md`): what an epoch is, when
//! to commit, which barrier strategy to pay for, and how durability is
//! reported all depend on a workload the crate cannot see, so baking them into
//! a primitive would be exactly the kind of policy D-8 refuses. The residue is
//! that every consumer would otherwise rediscover the same composition, and
//! the contracts involved (D-19's edge-triggered completion event, D-23's
//! non-covering flush, D-24's ring-wide barrier) are the kind that get learned
//! by deadlock or by data loss. This sample is the transfer vehicle: a worked
//! answer to "how do these go together", offered as a starting point you own.
//!
//! # Read [`contract`] first
//!
//! The sample's own durability contract is written down in [`contract`], and
//! it was written *before* any of the code that implements it. That ordering
//! is deliberate: the contract is this program's specification, phrased in its
//! own words rather than as a description of what the ring happens to do. The
//! code below has to satisfy it; it does not get to define it.
//!
//! # The modules, and what each one is for
//!
//! Roughly the order to read them.
//!
//! | Module | What it holds | The contract behind it |
//! |---|---|---|
//! | [`contract`] | What this log guarantees, does not guarantee, and assumes | written first, on purpose |
//! | [`record`] | The 28-byte header: magic, sequence, epoch, length, checksum | a torn record must be *detectable*, not survivable |
//! | [`append`] | The data path: compose into a registered arena slot, push one write | a registered buffer's address lives for the ring's life, so slots are reused only after their completion is observed |
//! | [`commit`] | Epochs, group commit, and the truthful `durable_through` | D-23: an unflagged flush does not cover preceding writes |
//! | [`event_loop`] | The multiplexed wait: ring, non-ring, shutdown | D-19: the completion event is an *edge*, so drain to empty every pass |
//! | [`reclaim`] | An `FSCTL` the ring cannot express, ordered against ring epochs | D-24 orders SQEs against SQEs, so this ordering is the log's job |
//! | [`checkpoint`] | The control plane: a second ring, delivered to the thread pool | D-21: a ring given to `EventDelivery` cannot also be waited on directly |
//! | [`replay`] | Reading the log back, including a negative control | a verifier that cannot fail proves nothing |
//! | [`strategy`] | All three commit strategies, implemented and measured | D-24 makes the choice a real fork with no free answer |
//!
//! # What one run does
//!
//! 1. Prints the contract, so a reader who only runs the program still learns
//!    what it does and does not promise.
//! 2. Appends records in epochs, committing each and waiting for it to become
//!    durable -- then appends a tail into an epoch it deliberately never
//!    commits, because a replay pass that never sees such a tail has not been
//!    asked the interesting question.
//! 3. Checkpoints on the pool, which is what authorises reclaiming a retired
//!    segment: log thread -> pool thread -> reclaim worker -> log thread, with
//!    the log thread blocking for none of it.
//! 4. Replays three ways: clean, with a torn tail, and with a byte corrupted
//!    inside the durable region that replay is *required* to catch.
//! 5. Runs the same workload under all three commit strategies and prints what
//!    each costs on this machine.
//!
//! # Two things it cannot show, said plainly
//!
//! - **Whether the ordering held on the device.** That is only observable
//!   across a power cut. Every durability claim here rests on D-23 and D-24
//!   plus the assumption, stated in [`contract`], that the device honours the
//!   flush.
//! - **A ranking of the three strategies.** Measured here they are
//!   indistinguishable, because the device flush dominates everything they
//!   differ about. [`strategy`] has the numbers and the reasoning.

mod append;
mod checkpoint;
mod commit;
mod contract;
mod event_loop;
mod reclaim;
mod record;
mod replay;
mod strategy;

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use append::{Appender, SLOT_LEN, SLOTS};
use checkpoint::Checkpointer;
use commit::{Committer, Epoch};
use contract::{CONTRACT, Clause};
use event_loop::{EventLoop, Woken};
use reclaim::Reclaimer;
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

/// How large the retired segment is (M14.1). It stands in for a previous
/// generation of the log that this run has superseded: a segmented log retires
/// whole segments, and reclaims them once the epoch that superseded them is
/// durable.
const RETIRED_LEN: u64 = 64 * 1024;

/// The byte the retired segment is filled with, so "was it reclaimed?" has an
/// answer that does not depend on what happened to be there.
const RETIRED_FILL: u8 = 0xA5;

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

/// Where the retired segment lives (M14.1).
///
/// A separate file on purpose. Reclaiming the *live* log would zero the very
/// records replay is about to check, which is not a subtlety of this sample but
/// the reason real logs are segmented: what gets reclaimed is a segment nothing
/// still needs, never the segment being appended to.
fn retired_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-log-{}-retired.log",
        std::process::id()
    ))
}

/// Where the checkpoint record lives (M14.2). Written on the control ring by
/// the thread pool, never by the log thread.
fn checkpoint_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-log-{}-checkpoint.bin",
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
    retired: &std::path::Path,
    checkpoint_path: &std::path::Path,
) -> io::Result<LogRun> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let handle = file.as_raw_handle();

    std::fs::write(retired, vec![RETIRED_FILL; RETIRED_LEN as usize])?;
    let checkpoint_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(checkpoint_path)?;

    let mut ring = IoRing::new(64, 128)?;
    let mut appender = Appender::new(&mut ring)?;
    let mut committer = Committer::new();

    // The reclaim worker is shared: the log thread waits on its handle, and a
    // pool thread is what asks it for work (M14.2).
    let reclaimer = Reclaimer::new(retired)?;
    let reclaim_handle = reclaimer.completion_handle().try_clone()?;
    let reclaimer = Arc::new(Mutex::new(reclaimer));

    // The control ring is a *second* ring, handed to the pool. It cannot be
    // the log's own: `EventDelivery` owns its ring's completion event, and a
    // second waiter on that event is what D-21 rules out.
    let checkpointer = Checkpointer::new(
        IoRing::new(8, 16)?,
        checkpoint_file.as_raw_handle(),
        Arc::clone(&reclaimer),
    )?;

    let events = EventLoop::new(&mut ring, &reclaim_handle)?;
    report.line(format_args!(
        "data path: Model B on this thread; control plane: Model A on a second ring \
         delivered to the thread pool"
    ));
    report.line(format_args!(
        "arena registered: {SLOTS} slots of {SLOT_LEN} bytes; \
         waiting on the ring's completion event alongside a reclaim event and a shutdown latch"
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
    let mut reclaimed_for: Option<Epoch> = None;
    let mut reclaim_failures: Vec<io::Error> = Vec::new();
    let mut collected = 0usize;
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

            // The ordering the ring cannot express (M14.1), now issued through
            // the control plane (M14.2). The retired segment is only dead once
            // a *checkpoint* records a watermark past it -- reclaim earlier and
            // a crash in between loses both copies.
            // `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` cannot help anywhere along this
            // chain: it orders SQEs against SQEs on one ring (D-24), and the
            // chain spans two rings and an `FSCTL` that is not an SQE at all.
            //
            // The assertion is the enforcement made checkable: move this block
            // above the `while !committer.is_durable(closed)` loop and it
            // fires immediately.
            if reclaimed_for.is_none() {
                assert!(
                    committer.is_durable(closed),
                    "a checkpoint must not be issued before the epoch it records is durable"
                );
                checkpointer.submit(closed, RETIRED_LEN / 2)?;
                reclaimed_for = Some(closed);
                report.line(format_args!(
                    "epoch {} is durable, so a checkpoint went to the control ring; \
                     a pool thread will authorise reclaiming the first {} bytes",
                    closed.0,
                    RETIRED_LEN / 2
                ));
            }
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

    // Phase 1 of the reclaim story: collect the reclaim that ran *alongside*
    // the appends. Measurement is blunt about what this does and does not
    // show. It shows the two kinds of work overlapping -- ring completions
    // kept being serviced for the whole time the checkpoint and the `FSCTL`
    // were running elsewhere. It does **not** show the reclaim handle being
    // load-bearing: removing it from the wait entirely costs this phase
    // nothing, because ring traffic wakes the loop anyway and the result is
    // collected on a ring arm. Measured, not assumed -- the sabotage was run,
    // and it passed.
    while collected < 1 {
        let (woken, popped) =
            events.pump(WAIT_MS, || drain(&mut ring, &mut appender, &mut committer))?;
        empty_wakes += usize::from(popped == 0);
        // Collected unconditionally, for the same reason the ring is drained
        // unconditionally: a result queued between the wait returning and this
        // line is still a result.
        collected += usize::from(collect_reclaim(
            report,
            &reclaimer,
            woken,
            &mut reclaim_failures,
        ));
        check_control_plane(&checkpointer)?;
    }

    // Phase 2 is where the handle has teeth. Quiesce the ring first, so every
    // append and commit has completed and *nothing on the log's ring will
    // signal again*, then issue a second checkpoint. The control ring's own
    // completions go to the pool, not to this wait, so the only handle that
    // can end this loop is the reclaim's: drop it from
    // `WaitForMultipleObjects` and the loop blocks for the full `WAIT_MS` with
    // a finished result sitting in the channel. That is the lost wakeup this
    // whole file exists to avoid, and unlike phase 1 it is reproducible on
    // demand.
    let mut attempts = 0;
    while appender.in_flight() > 0 || committer.in_flight() > 0 {
        attempts += 1;
        if attempts > QUIESCE_ATTEMPTS {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "outstanding operations never completed before the idle-path reclaim",
            ));
        }
        Batch::new(&mut ring).submit_and_wait(1, WAIT_MS)?;
        drain(&mut ring, &mut appender, &mut committer)?;
    }
    let watermark = committer
        .durable_through()
        .expect("at least one epoch was committed");
    checkpointer.submit(watermark, RETIRED_LEN)?;
    report.line(format_args!(
        "log ring is idle; a second checkpoint for epoch {} will authorise reclaiming the \
         rest of the retired segment ({RETIRED_LEN} bytes), and nothing but the reclaim \
         event can wake this wait",
        watermark.0
    ));
    while collected < 2 {
        let (woken, popped) =
            events.pump(WAIT_MS, || drain(&mut ring, &mut appender, &mut committer))?;
        empty_wakes += usize::from(popped == 0);
        collected += usize::from(collect_reclaim(
            report,
            &reclaimer,
            woken,
            &mut reclaim_failures,
        ));
        check_control_plane(&checkpointer)?;
    }

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

    // Conservation, once everything has quiesced (M16.2). Every append that
    // queued completed exactly once, and every token was claimed -- the second
    // half being what returns its arena slot. A slot leaked here would not
    // show up as an error at all: the arena would simply run dry `SLOTS`
    // appends later, somewhere else, with nothing pointing back to the cause.
    appender.contract().assert_quiescent();

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

    // Did the reclaim actually do anything? Asked of the bytes rather than of
    // the return code, because a call that reports success and changes nothing
    // is the failure worth catching. The endpoint is closed first so the read
    // cannot race the worker.
    let reclaimed_for = reclaimed_for.expect("a checkpoint was submitted");
    drop(reclaimer);

    // A failure on a pool thread is still a failure. Reporting it and then
    // continuing to the success line would be the same defect the checkpoint
    // path itself was fixed for: noticing a problem and not acting on it.
    let checkpoint_failures = checkpointer.failures();
    for failure in &checkpoint_failures {
        report.error_line(format_args!("control plane: {failure}"));
    }
    if !checkpoint_failures.is_empty() {
        return Err(io::Error::other(format!(
            "{} checkpoint failure(s) on pool threads",
            checkpoint_failures.len()
        )));
    }
    let checkpoint_watermark = checkpointer
        .durable_through()
        .expect("at least one checkpoint completed");
    report.line(format_args!(
        "control plane: {} checkpoint(s) completed on pool threads, durable through epoch {}",
        checkpointer.completed(),
        checkpoint_watermark.0
    ));
    drop(checkpointer);
    drop(checkpoint_file);
    if reclaim_failures.is_empty() {
        let bytes = std::fs::read(retired)?;
        let non_zero = bytes.iter().filter(|byte| **byte != 0).count();
        assert_eq!(
            non_zero, 0,
            "the reclaimed range still holds {non_zero} non-zero byte(s), so the FSCTL \
             reported success without reclaiming anything"
        );
        report.line(format_args!(
            "retired segment reads back as {} zero bytes: the first half reclaimed once \
             epoch {} was checkpointed, the rest on the idle path",
            bytes.len(),
            reclaimed_for.0
        ));
    } else {
        // Not fatal, and not smoothed over either: `FSCTL_SET_ZERO_DATA` is a
        // filesystem feature, and a volume that refuses it says nothing about
        // the ordering this item is demonstrating.
        report.error_line(format_args!(
            "{} reclaim FSCTL(s) failed on this volume; the ordering they demonstrate still \
             held, but the retired segment was not zeroed",
            reclaim_failures.len()
        ));
    }

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

/// Fail fast if the control plane reported a failure on a pool thread.
///
/// Without this, a failed checkpoint is not merely unreported -- it *hangs the
/// waiter*. The log thread is waiting for a reclaim that a pool thread will now
/// never authorise, so it sits until `WAIT_MS` and then blames the wait
/// ("no handle signalled") rather than the cause. Checking each pass turns a
/// 30-second timeout with a misleading message into an immediate, accurate one.
fn check_control_plane(checkpointer: &Checkpointer) -> io::Result<()> {
    let failures = checkpointer.failures();
    if failures.is_empty() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "control plane failed, so no reclaim will be authorised: {}",
        failures.join("; ")
    )))
}

/// Collect a finished reclamation if one is ready, reporting which arm of the
/// multiplexed wait it arrived on.
///
/// Called unconditionally rather than only on [`Woken::NonRing`], for the same
/// reason the ring is drained unconditionally: a result queued between the
/// wait returning and this call is still a result.
fn collect_reclaim<O: io::Write, E: io::Write>(
    report: &mut Report<O, E>,
    reclaimer: &Mutex<Reclaimer>,
    woken: Woken,
    failures: &mut Vec<io::Error>,
) -> bool {
    let done = {
        let mut reclaimer = reclaimer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reclaimer.take_completed()
    };
    let Some(done) = done else {
        return false;
    };
    let arm = match woken {
        Woken::NonRing => "reclaim",
        Woken::Ring => "ring",
        Woken::Shutdown => "shutdown",
    };
    match done.result {
        Ok(()) => report.line(format_args!(
            "reclaim for epoch {} finished on the {arm} arm: {} bytes zeroed",
            done.epoch.0, done.bytes
        )),
        Err(error) => {
            report.error_line(format_args!(
                "reclaim for epoch {} failed on the {arm} arm: {error}",
                done.epoch.0
            ));
            failures.push(error);
        }
    }
    true
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

/// Format a duration in microseconds, which is the resolution these
/// measurements actually have.
fn micros(duration: std::time::Duration) -> String {
    format!("{} us", duration.as_micros())
}

/// Run the same log three ways, once per commit strategy (M14.3), and measure
/// what each one costs on this machine (M14.4).
///
/// Every strategy produces the same records in the same order, so the check
/// that matters is that all three replay clean. Which one to *pay for* is a
/// separate question, and M14.4 is what answers it with numbers.
fn compare_strategies<O: io::Write, E: io::Write>(
    report: &mut Report<O, E>,
    directory: &std::path::Path,
) -> io::Result<()> {
    const EPOCHS: usize = 32;
    const PER_EPOCH: usize = 64;

    report.line(format_args!(""));
    report.line(format_args!(
        "commit strategies: {EPOCHS} epochs of {PER_EPOCH} records, each run replayed"
    ));
    report.line(format_args!(
        "  these numbers describe THIS machine and THIS device. They are printed rather than \
         quoted in the docs because quoting ours would be misleading."
    ));

    let payload = b"strategy comparison record payload";
    let mut reference: Option<(&'static str, Vec<u8>)> = None;
    let mut throughputs: Vec<f64> = Vec::new();
    for strategy in strategy::CommitStrategy::ALL {
        let path = directory.join(format!(
            "windows-ioring-sys-epoch-log-{}-{}.log",
            std::process::id(),
            strategy.name()
        ));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;

        let outcome = strategy::run(strategy, file.as_raw_handle(), EPOCHS, PER_EPOCH, payload);
        drop(file);
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(error);
            }
        };

        // Replayed with the same verifier the log itself uses, because a
        // strategy that is fast and wrong is not a strategy.
        let bytes = std::fs::read(&path)?;
        assert_eq!(
            outcome.bytes as usize,
            bytes.len(),
            "{} wrote {} bytes but accounted for {}",
            strategy.name(),
            bytes.len(),
            outcome.bytes
        );
        let outcome_replay =
            replay::replay(&bytes, outcome.durable_through, outcome.records, |index| {
                let mut expected = Vec::with_capacity(payload.len());
                expected.extend_from_slice(payload);
                let _ = index;
                expected
            });
        let _ = std::fs::remove_file(&path);

        assert!(
            outcome_replay.is_clean(),
            "{} produced a log that does not replay clean: {:?}",
            strategy.name(),
            outcome_replay.violations
        );

        // The cross-strategy invariant, and the one with real teeth. Replay
        // checks a log against itself; this checks the three strategies
        // against *each other*, so a dropped record, a wrong offset, or an
        // epoch tagged to the wrong commit shows up as a byte difference
        // rather than passing three times independently.
        //
        // What it cannot check is the thing the strategies actually differ
        // about: whether the ordering held on the *device*. That is only
        // observable across a power cut, and no in-process check substitutes
        // for it -- which is why the strategies are argued from D-23 and D-24
        // rather than from this run passing.
        match &reference {
            None => reference = Some((strategy.name(), bytes)),
            Some((first, expected)) => assert_eq!(
                &bytes,
                expected,
                "{} produced a different log than {first}; all three must write the same bytes",
                strategy.name()
            ),
        }
        report.line(format_args!(
            "  {:<18} {:>8.0} rec/s  commit p50 {:>7}  p99 {:>7}  max {:>7}  \
             append stall {:>7}  -- pays {}",
            outcome.strategy.name(),
            outcome.throughput(),
            micros(outcome.commit_quantile(0.50)),
            micros(outcome.commit_quantile(0.99)),
            micros(outcome.commit_quantile(1.0)),
            micros(outcome.append_stall),
            outcome.strategy.cost()
        ));
        throughputs.push(outcome.throughput());
    }

    // The reading, computed from this run rather than asserted from theory.
    //
    // The spread across strategies is only meaningful next to the spread the
    // *same* strategy shows between runs, so the program says so instead of
    // declaring a winner. On the machine this was written on the two are the
    // same size, and the reason is visible in the numbers above: every
    // strategy pays exactly one device flush per epoch, that flush is hundreds
    // of microseconds, and everything the strategies actually differ about --
    // ring-side stalls, an extra host round trip -- lands in the tens. The
    // distinction D-24 draws is real; on this device it is two orders of
    // magnitude below the dominant term.
    //
    // That is not a licence to pick the cheapest-looking one. A device with a
    // fast flush, a log that commits far more often, or an arena under real
    // pressure moves the balance, and the only way to know is to run this on
    // the machine in question.
    let low = throughputs.iter().copied().fold(f64::INFINITY, f64::min);
    let high = throughputs.iter().copied().fold(0.0, f64::max);
    if low > 0.0 {
        report.line(format_args!(
            "  spread across strategies: {:.2}x. Run this twice: if the run-to-run spread of one \
             strategy is the same size, the choice is dominated by the device flush that all \
             three pay once per epoch.",
            high / low
        ));
    }
    Ok(())
}

fn main() {
    let mut report = Report::new(io::stdout(), io::stderr());
    report_contract(&mut report);
    report.line(format_args!(""));

    let path = log_path();
    let retired = retired_path();
    let checkpoint = checkpoint_path();
    let outcome = run_log(&mut report, &path, &retired, &checkpoint)
        .and_then(|run| {
            report.line(format_args!(""));
            verify(&mut report, &path, &run)
        })
        .and_then(|()| compare_strategies(&mut report, &std::env::temp_dir()));
    match outcome {
        Ok(()) => report.line(format_args!(
            "the log kept its contract, and the verifier that says so was shown to be able to fail"
        )),
        Err(error) => {
            report.error_line(format_args!("epoch log failed: {error}"));
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&retired);
            let _ = std::fs::remove_file(&checkpoint);
            std::process::exit(1);
        }
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&retired);
    let _ = std::fs::remove_file(&checkpoint);
}
