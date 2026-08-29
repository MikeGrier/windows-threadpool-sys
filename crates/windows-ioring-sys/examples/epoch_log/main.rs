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
//! M13.1 delivered the contract, M13.2 the record format and append path;
//! M13.3 (this) adds epoch bookkeeping and group commit, which is what makes
//! the contract's guarantee answerable. The pieces that finish it:
//!
//! - **M13.4** -- the event loop: the ring's completion event multiplexed with
//!   a shutdown event, draining to empty on every pass.
//! - **M13.5** -- replay and verify, which is the only part that can actually
//!   catch a durability bug.

mod append;
mod commit;
mod contract;
mod record;

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;

use append::{Appender, SLOT_LEN, SLOTS};
use commit::{Committer, Epoch};
use contract::{CONTRACT, Clause};
use record::Sequence;
use windows_ioring_sys::{Batch, IoRing};

/// How many records this demonstration appends.
const RECORDS: usize = 24;

/// Records per epoch. Small so the demonstration shows several commits; a real
/// log sizes this against its latency target, since the commit is what costs.
const EPOCH_SIZE: usize = 6;

/// Bound on the wait for a commit, so a stuck example fails instead of hanging.
const WAIT_MS: u32 = 30_000;

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

/// Append `RECORDS` records in epochs of `EPOCH_SIZE`, committing each one and
/// waiting for it to become durable before moving on.
///
/// This is M13.3's deliverable in one function: records join the open epoch,
/// closing an epoch pushes one covering flush, and observing that flush's
/// completion is what makes `is_durable` start answering `true`.
fn run_log<O: io::Write, E: io::Write>(
    report: &mut Report<O, E>,
    path: &std::path::Path,
) -> io::Result<()> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(64, 128)?;
    let mut appender = Appender::new(&mut ring)?;
    let mut committer = Committer::new();
    report.line(format_args!(
        "arena registered: {SLOTS} slots of {SLOT_LEN} bytes"
    ));

    let mut appended = 0;
    while appended < RECORDS {
        let epoch = committer.open_epoch();
        let payload = payload_for(appended);
        match appender.append(&mut ring, handle, epoch, &payload) {
            Ok(_sequence) => appended += 1,
            // Every slot is in flight. This is the arena working as intended,
            // not an error: drain one and try again.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                drain(&mut ring, &mut appender, &mut committer)?;
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
            await_durable(&mut ring, &mut appender, &mut committer, closed)?;
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

    // Drain to empty before returning, so every arena slot is released and
    // nothing is outstanding when the ring closes.
    while appender.in_flight() > 0 || committer.in_flight() > 0 {
        drain(&mut ring, &mut appender, &mut committer)?;
    }

    let durable_through = committer
        .durable_through()
        .expect("at least one epoch was committed");
    report.line(format_args!(
        "appended {appended} records (sequences 0..{}) across {} epochs; durable through epoch {}",
        appender.next_sequence().0,
        committer.open_epoch().0,
        durable_through.0
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

    verify_on_disk(report, path, file, appended, durable_through)
}

/// Read the log back and decode it.
///
/// Not the replay pass -- M13.5 owns verifying the *contract*, including the
/// torn tail. This checks the weaker thing M13.3 can already claim: that the
/// records are in the format described, and that each one carries the epoch it
/// was appended into.
fn verify_on_disk<O: io::Write, E: io::Write>(
    report: &mut Report<O, E>,
    path: &std::path::Path,
    file: std::fs::File,
    appended: usize,
    durable_through: Epoch,
) -> io::Result<()> {
    drop(file);
    let bytes = std::fs::read(path)?;
    let mut decoded = 0;
    let mut cursor = 0;
    while cursor < bytes.len() {
        match record::decode(&bytes[cursor..]) {
            Ok(record) => {
                assert_eq!(
                    record.sequence,
                    Sequence(decoded as u64),
                    "records must decode in sequence order on a log written by one appender"
                );
                assert_eq!(
                    record.payload,
                    payload_for(decoded),
                    "a decoded payload must be byte-identical to what was appended"
                );
                assert_eq!(
                    record.epoch,
                    Epoch((decoded / EPOCH_SIZE) as u64),
                    "a record must carry the epoch that was open when it was appended"
                );
                assert!(
                    record.epoch <= durable_through,
                    "every record here belongs to a committed epoch"
                );
                cursor += record.total_len;
                decoded += 1;
            }
            Err(reason) => {
                report.line(format_args!(
                    "stopped decoding at byte {cursor}: {reason:?}"
                ));
                break;
            }
        }
    }
    report.line(format_args!(
        "read back {} bytes and decoded {decoded} of {appended} records, \
         every one stamped with its epoch",
        bytes.len()
    ));
    assert_eq!(
        decoded, appended,
        "every appended record must decode cleanly from a log that was never truncated"
    );
    Ok(())
}

/// Block until `epoch` is durable, draining completions while we wait.
///
/// The blocking wait here is the *fused* submit-and-wait: with nothing new
/// queued its only effect is to park until a completion arrives. M13.4
/// replaces it with the multiplexed wait, which is what a log that also has to
/// service non-ring handles needs -- the shape changes, the accounting below
/// does not.
fn await_durable(
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
    match run_log(&mut report, &path) {
        Ok(()) => report.line(format_args!("log written to {}", path.display())),
        Err(error) => {
            report.error_line(format_args!("epoch log failed: {error}"));
            let _ = std::fs::remove_file(&path);
            std::process::exit(1);
        }
    }

    report.line(format_args!(""));
    report.line(format_args!(
        "Still to come: M13.4 runs the multiplexed event loop, M13.5 replays and verifies."
    ));
    let _ = std::fs::remove_file(&path);
}
