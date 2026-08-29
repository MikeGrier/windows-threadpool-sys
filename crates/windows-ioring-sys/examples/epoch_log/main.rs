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
//! M13.1 delivered the contract; M13.2 (this) adds the record format and the
//! append path. The pieces that finish it:
//!
//! - **M13.3** -- epoch bookkeeping and group commit, so a caller can await
//!   "epoch N is durable" and get a truthful answer.
//! - **M13.4** -- the event loop: the ring's completion event multiplexed with
//!   a shutdown event, draining to empty on every pass.
//! - **M13.5** -- replay and verify, which is the only part that can actually
//!   catch a durability bug.

mod append;
mod contract;
mod record;

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;

use append::{Appender, SLOT_LEN, SLOTS};
use contract::{CONTRACT, Clause};
use record::Sequence;
use windows_ioring_sys::IoRing;

/// How many records this demonstration appends.
const RECORDS: usize = 24;

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

/// Append `RECORDS` records and drain every completion.
///
/// This is the whole of M13.2: records composed into the registered arena and
/// pushed. Nothing here is durable yet -- that is M13.3's commit -- and saying
/// so out loud is the point, because "the write completed" is exactly the
/// thing [`contract`] warns against reading as durability.
fn append_records<O: io::Write, E: io::Write>(
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
    report.line(format_args!(
        "arena registered: {SLOTS} slots of {SLOT_LEN} bytes"
    ));

    let mut appended = 0;
    let mut completed = 0;
    while appended < RECORDS {
        let payload = payload_for(appended);
        match appender.append(&mut ring, handle, &payload) {
            Ok(_sequence) => appended += 1,
            // Every slot is in flight. This is the arena working as intended,
            // not an error: drain one and try again.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                completed += drain(&mut ring, &mut appender)?;
            }
            Err(error) => return Err(error),
        }
    }

    // Drain to empty before returning, so every arena slot is released and
    // nothing is outstanding when the ring closes.
    while appender.in_flight() > 0 {
        completed += drain(&mut ring, &mut appender)?;
    }

    report.line(format_args!(
        "appended {appended} records, {completed} write completions observed; \
         next sequence would be {}",
        appender.next_sequence().0
    ));
    report.line(format_args!(
        "none of them are durable yet: a write completing means the kernel took the bytes, \
         not that they survive power loss (M13.3 adds the commit)"
    ));

    // Read the bytes back and decode them, so "the records are in the format
    // above" is checked rather than asserted. This is not the replay pass --
    // M13.5 owns verifying the *contract*, including the torn tail. All this
    // shows is that the append path produced what it claims to produce.
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
        "read back {} bytes and decoded {decoded} of {appended} records",
        bytes.len()
    ));
    assert_eq!(
        decoded, appended,
        "every appended record must decode cleanly from a log that was never truncated"
    );
    Ok(())
}

/// Pop every completion currently available, handing each to the appender.
fn drain(ring: &mut IoRing, appender: &mut Appender) -> io::Result<usize> {
    let mut popped = 0;
    while let Some(completion) = ring.try_pop()? {
        if appender.claim(&completion)? {
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
    match append_records(&mut report, &path) {
        Ok(()) => report.line(format_args!("log written to {}", path.display())),
        Err(error) => {
            report.error_line(format_args!("append failed: {error}"));
            let _ = std::fs::remove_file(&path);
            std::process::exit(1);
        }
    }

    report.line(format_args!(""));
    report.line(format_args!(
        "Still to come: M13.3 commits epochs, M13.4 runs the event loop, M13.5 replays \
         and verifies."
    ));
    let _ = std::fs::remove_file(&path);
}
