// Copyright (c) 2026 Mike Grier
//! The delay events (DI-12.1.1, DI-D-26): what a seal's flushes report, how long they are said to
//! have taken, and that nothing is built or read from the clock for a sink that does not want it.

use std::sync::{Arc, Mutex};

use super::{A, B, DEFAULT, DEFAULT_LINEAGE, FULL, One, Ops, Sealing, quiet, submitted};
use crate::clock::StampClock;
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink};
use crate::domains::DomainId;
use crate::durability::Routing;
use crate::test_support::{ManualClock, Ticks};
use crate::types::FlushDomain;

/// A sink that keeps each event it is given, as text, and wants only the kinds it was told to.
struct Only {
    kinds: Vec<DiagnosticKind>,
    seen: Mutex<Vec<String>>,
}

impl Only {
    fn wanting(kinds: &[DiagnosticKind]) -> Arc<Self> {
        Arc::new(Self {
            kinds: kinds.to_vec(),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn everything() -> Arc<Self> {
        Self::wanting(&[
            DiagnosticKind::FlushIssued,
            DiagnosticKind::FlushAnswered,
            DiagnosticKind::SealWaited,
        ])
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("the record").clone()
    }

    /// What has been seen, and forgotten.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.seen.lock().expect("the record"))
    }
}

impl DiagnosticSink for Only {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        self.kinds.contains(&kind)
    }

    fn report(&self, event: &Diagnostic<'_>) {
        assert!(self.wants(event.kind()), "reported an event not wanted");
        self.seen
            .lock()
            .expect("the record")
            .push(event.to_string());
    }
}

const PROVIDER: Routing = Routing {
    default: false,
    provider: true,
};

fn core(clock: &ManualClock, sink: &Arc<Only>) -> One<ManualClock> {
    One::reporting(clock.clone(), sink.clone())
}

fn prefix(core: &One<ManualClock>) -> String {
    format!(
        "instance {} lineage {DEFAULT_LINEAGE} seal through",
        core.instance.number()
    )
}

#[test]
fn a_flush_issued_and_its_answer_report_how_many_were_asked_and_how_long_one_took() {
    let (clock, sink) = (ManualClock::at(1_000), Only::everything());
    let mut core = core(&clock, &sink);
    let mut ops = Ops::new();
    let write = ops.push(&mut core, 1, A, DEFAULT);
    core.completed(write, FULL);
    let due = submitted(core.seal(1));
    assert_eq!(due.flushes.len(), 1);
    let at = prefix(&core);
    assert_eq!(
        sink.take(),
        [format!("{at} 1: issued flushes of 1 files and 0 domains")]
    );
    clock.advance(25);
    core.flushed(1, A, Ok(()));
    assert_eq!(
        sink.take(),
        [format!("{at} 1: flush of file 1 succeeded after 25ns")]
    );
}

#[test]
fn a_failed_flush_is_reported_as_failed_with_how_long_it_took() {
    let (clock, sink) = (ManualClock::at(0), Only::everything());
    let mut core = core(&clock, &sink);
    let mut ops = Ops::new();
    let write = ops.push(&mut core, 1, B, DEFAULT);
    core.completed(write, FULL);
    submitted(core.seal(1));
    sink.take();
    clock.advance(7);
    core.flushed(1, B, Err(std::io::Error::from_raw_os_error(1117)));
    assert_eq!(
        sink.take(),
        [format!(
            "{} 1: flush of file 2 failed after 7ns",
            prefix(&core)
        )]
    );
}

#[test]
fn a_providers_domain_is_reported_by_the_name_the_consumer_gave_it() {
    let (clock, sink) = (ManualClock::at(0), Only::everything());
    let mut core = core(&clock, &sink);
    core.serve(Arc::from([DomainId(4)]));
    let mut ops = Ops::new();
    let write = ops.push_in(&mut core, 1, A, PROVIDER, &[4]);
    core.completed(write, FULL);
    let due = submitted(core.seal(1));
    assert_eq!(due.calls.len(), 1);
    let at = prefix(&core);
    assert_eq!(
        sink.take(),
        [format!("{at} 1: issued flushes of 0 files and 1 domains")]
    );
    clock.advance(40);
    core.provided(
        DEFAULT_LINEAGE,
        due.calls[0].seal,
        DomainId(4),
        &FlushDomain::new(b"ssd-4".to_vec()),
        Ok(()),
    );
    assert_eq!(
        sink.take(),
        [format!(
            "{at} 1: flush of domain \"ssd-4\" succeeded after 40ns"
        )]
    );
}

#[test]
fn a_seal_with_a_file_and_a_domain_reports_both_parts_each_with_its_own_time() {
    let (clock, sink) = (ManualClock::at(0), Only::everything());
    let mut core = core(&clock, &sink);
    core.serve(Arc::from([DomainId(1)]));
    let mut ops = Ops::new();
    let mixed = ops.push_in(
        &mut core,
        1,
        A,
        Routing {
            default: true,
            provider: true,
        },
        &[1, 9],
    );
    core.completed(mixed, FULL);
    let due = submitted(core.seal(1));
    let at = prefix(&core);
    assert_eq!(
        sink.take(),
        [format!("{at} 1: issued flushes of 1 files and 1 domains")]
    );
    clock.advance(10);
    core.flushed(1, A, Ok(()));
    clock.advance(5);
    core.provided(
        DEFAULT_LINEAGE,
        due.calls[0].seal,
        DomainId(1),
        &FlushDomain::new(b"one".to_vec()),
        Ok(()),
    );
    assert_eq!(
        sink.take(),
        [
            format!("{at} 1: flush of file 1 succeeded after 10ns"),
            format!("{at} 1: flush of domain \"one\" succeeded after 15ns"),
        ]
    );
}

#[test]
fn a_seal_that_waited_for_its_writes_reports_how_long_and_then_issues_its_flushes() {
    let (clock, sink) = (ManualClock::at(500), Only::everything());
    let mut core = core(&clock, &sink);
    let mut ops = Ops::new();
    let write = ops.push(&mut core, 1, A, DEFAULT);
    let sealing = core.seal(1);
    assert!(
        matches!(&sealing, Sealing::Submitted(due) if quiet(due) && due.flushes.is_empty()),
        "the write is in flight, so the seal waits"
    );
    assert!(sink.take().is_empty(), "nothing is reported while it waits");
    clock.advance(30);
    core.completed(write, FULL);
    let at = prefix(&core);
    assert_eq!(
        sink.take(),
        [
            format!("{at} 1: waited 30ns for its writes"),
            format!("{at} 1: issued flushes of 1 files and 0 domains"),
        ]
    );
    assert!(
        core.seal_waiting.is_empty(),
        "the stamp is taken out when reported"
    );
}

#[test]
fn a_seal_that_did_not_wait_reports_no_wait() {
    let (clock, sink) = (ManualClock::at(0), Only::everything());
    let mut core = core(&clock, &sink);
    let mut ops = Ops::new();
    let write = ops.push(&mut core, 1, A, DEFAULT);
    core.completed(write, FULL);
    submitted(core.seal(1));
    let seen = sink.seen();
    assert_eq!(seen.len(), 1, "only the issue: {seen:?}");
    assert!(seen[0].contains("issued flushes"), "{seen:?}");
}

#[test]
fn a_seal_that_waited_behind_another_is_timed_from_when_it_was_first_seen_to_wait() {
    let (clock, sink) = (ManualClock::at(0), Only::everything());
    let mut core = core(&clock, &sink);
    let mut ops = Ops::new();
    let first = ops.push(&mut core, 1, A, DEFAULT);
    let second = ops.push(&mut core, 2, B, DEFAULT);
    submitted(core.seal(1));
    clock.advance(10);
    submitted(core.seal(2));
    clock.advance(20);
    core.completed(first, FULL);
    let at = prefix(&core);
    // The first seal waited 30ns in all; the second is still behind the write at epoch 2.
    assert_eq!(
        sink.take(),
        [
            format!("{at} 1: waited 30ns for its writes"),
            format!("{at} 1: issued flushes of 1 files and 0 domains"),
        ]
    );
    clock.advance(5);
    core.completed(second, FULL);
    assert_eq!(
        sink.take(),
        [
            format!("{at} 2: waited 25ns for its writes"),
            format!("{at} 2: issued flushes of 1 files and 0 domains"),
        ]
    );
}

#[test]
fn a_sink_that_wants_nothing_builds_nothing_and_reads_no_clock() {
    let (clock, sink) = (Ticks::default(), Only::wanting(&[]));
    let mut core = One::reporting(clock.clone(), sink.clone());
    let mut ops = Ops::new();
    let write = ops.push(&mut core, 1, A, DEFAULT);
    let first = core.seal(1);
    assert!(matches!(first, Sealing::Submitted(_)));
    core.completed(write, FULL);
    core.flushed(1, A, Ok(()));
    assert!(sink.seen().is_empty());
    assert_eq!(
        clock.now(),
        0,
        "the core read the clock though no sink wanted a delay event"
    );
}

#[test]
fn a_sink_that_wants_only_the_issue_stamps_nothing_for_the_answer() {
    let (clock, sink) = (
        ManualClock::at(0),
        Only::wanting(&[DiagnosticKind::FlushIssued]),
    );
    let mut core = core(&clock, &sink);
    let mut ops = Ops::new();
    let write = ops.push(&mut core, 1, A, DEFAULT);
    core.completed(write, FULL);
    submitted(core.seal(1));
    assert!(
        core.flush_issued.is_empty(),
        "nothing kept for an unwanted answer"
    );
    core.flushed(1, A, Ok(()));
    assert_eq!(sink.seen().len(), 1);
}

#[test]
fn an_answer_is_dropped_from_what_is_kept_when_it_arrives() {
    let (clock, sink) = (ManualClock::at(0), Only::everything());
    let mut core = core(&clock, &sink);
    let mut ops = Ops::new();
    let write = ops.push(&mut core, 1, A, DEFAULT);
    core.completed(write, FULL);
    submitted(core.seal(1));
    assert_eq!(core.flush_issued.len(), 1);
    core.flushed(1, A, Ok(()));
    assert!(core.flush_issued.is_empty());
    assert!(core.seal_waiting.is_empty());
}

#[test]
fn a_flush_answered_after_its_lineage_ended_leaves_nothing_kept_and_reports_nothing() {
    let (clock, sink) = (ManualClock::at(0), Only::everything());
    let mut core = core(&clock, &sink);
    let minted = core.mint(None);
    let mut ops = Ops::new();
    let write = ops.push_to(&mut core, minted, 1, A, DEFAULT, &[]);
    core.completed(write, FULL);
    let sealed = core.0.seal(minted, 1);
    let Sealing::Submitted(_) = sealed else {
        panic!("a seal");
    };
    sink.take();
    core.0.end(minted);
    core.0.flushed(minted, 1, A, Ok(()));
    assert!(core.flush_issued.is_empty(), "{:?}", core.flush_issued);
    assert!(sink.take().is_empty());
}

#[test]
fn the_events_name_the_instance_and_the_lineage() {
    let (clock, sink) = (ManualClock::at(0), Only::everything());
    let mut core = core(&clock, &sink);
    let minted = core.mint(None);
    let mut ops = Ops::new();
    let write = ops.push_to(&mut core, minted, 1, A, DEFAULT, &[]);
    core.completed(write, FULL);
    core.0.seal(minted, 1);
    let text = sink.take();
    assert_eq!(
        text,
        [format!(
            "instance {} lineage {minted} seal through 1: issued flushes of 1 files and 0 domains",
            core.instance.number()
        )]
    );
    assert_ne!(minted, DEFAULT_LINEAGE);
}
