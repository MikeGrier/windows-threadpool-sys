// Copyright (c) 2026 Mike Grier
//! The poll engine: copy, probe, report, and a stop point in the third (TW-D-7).
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the poll on the thread pool (TW-3.1) runs it")
)]

use std::io;

use windows_overlapped_io_sys::EndpointObserver;

use super::probe::{Change, Outcome};
use super::{Entry, SnapshotItem, Watcher};
use crate::chunked::Chunked;
use crate::sq::{Attention, SqEntry, SqPushError, SqWriter, Terminal};

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

/// How many changes the report pass writes per acquisition of the roster lock. Provisional: TW-2.8
/// chooses.
const REPORT_BATCH: usize = 256;

/// How a run of the engine ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    /// Everything was written, poll-done included.
    Complete,
    /// The SQ was full. Running again continues from the entry that was refused.
    Stopped,
}

enum Phase {
    /// Nothing in flight: the next run starts a poll.
    Idle,
    /// A poll stopped in its report pass: the next run continues it.
    Reporting,
}

/// One poll at a time, which outlives a stop: its buffers are kept between polls, so there is no
/// allocation after warm-up, and the index of the next change to report is kept between runs.
pub(crate) struct Engine<T> {
    snapshot: Chunked<SnapshotItem>,
    changed: Chunked<Change>,
    writer: SqWriter<T>,
    // Into `changed`; equal to its length once every change is written and poll-done is not.
    next: usize,
    phase: Phase,
}

impl<T: Clone> Engine<T> {
    /// An engine for `watcher`, which every later run must be given.
    pub(crate) fn new(watcher: &Watcher<T>) -> Self {
        Self {
            snapshot: Chunked::new(),
            changed: Chunked::new(),
            writer: watcher.sq_writer(),
            next: 0,
            phase: Phase::Idle,
        }
    }

    /// Start a poll, or continue one that stopped, and run it until it completes or the SQ fills.
    pub(crate) fn run(&mut self, watcher: &Watcher<T>) -> RunOutcome {
        self.begin(watcher, EndpointObserver::file_len);
        self.finish(watcher, REPORT_BATCH, || {})
    }

    // Starts a poll if none is in flight: the copy and the probe. The read is the probe itself, so
    // that a test can make one fail.
    fn begin(&mut self, watcher: &Watcher<T>, read: impl Fn(&EndpointObserver) -> io::Result<u64>) {
        if matches!(self.phase, Phase::Idle) {
            self.snapshot.clear();
            self.changed.clear();
            watcher.snapshot(&mut self.snapshot);
            watcher.probe_with(&mut self.snapshot, &mut self.changed, read);
            self.next = 0;
            self.phase = Phase::Reporting;
        }
    }

    fn finish(&mut self, watcher: &Watcher<T>, batch: usize, between: impl FnMut()) -> RunOutcome {
        let outcome = watcher.report(&self.changed, &mut self.next, &self.writer, batch, between);
        if outcome == RunOutcome::Complete {
            self.phase = Phase::Idle;
        }
        outcome
    }
}

/// Write `change` for `entry`, and on success make what it reported the entry's state: its last
/// known length, or terminal. A write the SQ refuses hands the unwritten entry back.
fn write_change<T: Clone>(
    entry: &mut Entry<T>,
    change: Change,
    writer: &SqWriter<T>,
) -> Result<(), SqEntry<T>> {
    let sq_entry = match change.outcome {
        Outcome::Changed { current } => SqEntry::Attention(Attention {
            value: entry.value.clone(),
            last_known: change.last_known,
            current,
        }),
        Outcome::Terminal { win32_code } => SqEntry::Terminal(Terminal {
            value: entry.value.clone(),
            win32_code,
            last_known: change.last_known,
        }),
    };
    match writer.push(sq_entry) {
        Ok(()) => {
            match change.outcome {
                Outcome::Changed { current } => entry.last_known = current,
                Outcome::Terminal { win32_code } => {
                    entry.mark_terminal(win32_code);
                }
            }
            Ok(())
        }
        Err(SqPushError::Full(unwritten)) => Err(unwritten),
        Err(SqPushError::Closed(_)) => unreachable!("the watcher holds the SQ's reader"),
    }
}

impl<T: Clone> Watcher<T> {
    /// Write the changes from `*next` on, and then poll-done, to `writer`, a batch of changes per
    /// acquisition of the roster lock. `*next` follows the changes written or skipped, so a stop
    /// leaves it at the first one not written.
    ///
    /// A registration dropped since the probe is skipped. A write that succeeds makes the length it
    /// reported the registration's last known one, or marks it terminal. A write the SQ refuses
    /// stops the pass without waiting; the `T` cloned for it is dropped after the lock is released.
    fn report(
        &self,
        changed: &Chunked<Change>,
        next: &mut usize,
        writer: &SqWriter<T>,
        batch: usize,
        mut between: impl FnMut(),
    ) -> RunOutcome {
        assert!(batch > 0, "a batch of nothing never finishes");
        loop {
            let mut refused = None;
            let mut listed = 0;
            let exhausted = {
                let mut roster = self.shared.roster();
                loop {
                    if listed == batch {
                        break false;
                    }
                    let Some(&change) = changed.get(*next) else {
                        break true;
                    };
                    if let Some(entry) = roster.entries.get_mut(&change.id)
                        && entry.terminal.is_none()
                        && let Err(unwritten) = write_change(entry, change, writer)
                    {
                        refused = Some(unwritten);
                        break false;
                    }
                    *next += 1;
                    listed += 1;
                }
            };
            if refused.is_some() {
                drop(refused);
                return RunOutcome::Stopped;
            }
            if exhausted {
                return match writer.push(SqEntry::PollDone) {
                    Ok(()) => RunOutcome::Complete,
                    Err(SqPushError::Full(_)) => RunOutcome::Stopped,
                    Err(SqPushError::Closed(_)) => {
                        unreachable!("the watcher holds the SQ's reader")
                    }
                };
            }
            between();
        }
    }
}
