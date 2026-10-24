// Copyright (c) 2026 Mike Grier
//! The seal and flush pairs (DI-19.2.2, DI-D-87): each begins as the core builds it and ends on
//! every path it can leave by, held to the pairing rule by `PairingChecker` and not by a copy of
//! it here.

use std::sync::{Arc, Mutex};

use super::{Core, KEY, Rig, nz};
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink};
use crate::memory::{
    Fault, FaultScript, MemoryBackend, MemoryTarget, ScriptOp, When, scripted_fault,
};
use crate::oracle::{PairingChecker, PairingViolation};
use crate::provider::DomainOutcome;
use crate::test_support::Ticks;
use crate::types::{DurabilityRequest, Epoch, FlushDomain, Resolution};

/// A sink that wants the kinds it is given, checks every event it is handed against the pairing
/// rule, and keeps the pairs' lines. It records under a mutex it holds only for that.
struct Checked {
    wants: Vec<DiagnosticKind>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    checker: PairingChecker,
    lines: Vec<String>,
    violation: Option<PairingViolation>,
}

impl Checked {
    fn wanting(wants: &[DiagnosticKind]) -> Arc<Self> {
        Arc::new(Self {
            wants: wants.to_vec(),
            state: Mutex::new(State::default()),
        })
    }

    fn pairs() -> Arc<Self> {
        Self::wanting(&[DiagnosticKind::Seal, DiagnosticKind::Flush])
    }

    fn lines(&self) -> Vec<String> {
        let state = self.state.lock().expect("the record");
        assert_eq!(state.violation, None, "{:?}", state.lines);
        state.lines.clone()
    }

    /// The stream so far keeps the rule, with nothing left open.
    fn finished(&self) {
        let mut state = self.state.lock().expect("the record");
        assert_eq!(state.violation, None, "{:?}", state.lines);
        let checker = std::mem::take(&mut state.checker);
        checker.finish(&[]).expect("every pair ended");
    }
}

impl DiagnosticSink for Checked {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        self.wants.contains(&kind)
    }

    fn report(&self, event: &Diagnostic<'_>) {
        let mut state = self.state.lock().expect("the record");
        if event.pairing().is_some() {
            state.lines.push(event.to_string());
        }
        if state.violation.is_none()
            && let Err(violation) = state.checker.observe(event)
        {
            state.violation = Some(violation);
        }
    }
}

fn domain(name: &str) -> FlushDomain {
    FlushDomain::new(name.as_bytes())
}

/// A rig reporting to `sink`, over a backend doing what `script` says, with `KEY` declared in
/// `declared`, and a provider serving `served` if any.
fn rig(sink: &Arc<Checked>, script: FaultScript, served: &[&str], declared: &[&str]) -> Rig {
    let sink: Arc<dyn DiagnosticSink> = Arc::clone(sink) as Arc<dyn DiagnosticSink>;
    let core = if served.is_empty() {
        Core::new(Ticks::default(), sink)
    } else {
        Core::with_provider(
            Ticks::default(),
            sink,
            served.iter().map(|name| domain(name)).collect(),
        )
        .expect("no repeats")
    };
    let mut rig = Rig::over(core);
    rig.backend = MemoryBackend::new(nz(8), nz(16)).with_script(script);
    rig.attach(
        KEY,
        MemoryTarget::new().with_domains(declared.iter().map(|name| domain(name))),
    );
    rig
}

fn plain(sink: &Arc<Checked>) -> Rig {
    rig(sink, FaultScript::new(), &[], &[])
}

/// Which lines say what: the `n`th line contains `text`.
fn says(lines: &[String], expected: &[&str]) {
    assert_eq!(lines.len(), expected.len(), "{lines:#?}");
    for (line, text) in lines.iter().zip(expected) {
        assert!(
            line.contains(text),
            "{line:?} should say {text:?}; all: {lines:#?}"
        );
    }
}

#[test]
fn a_durable_seal_is_a_seal_pair_around_one_flush_pair() {
    let sink = Checked::pairs();
    let mut rig = plain(&sink);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    assert_eq!(rig.seal(1), DurabilityRequest::Submitted);
    rig.run();
    rig.entries();
    says(
        &sink.lines(),
        &[
            "seal through 1 (pair",
            "flush of file 1 (pair",
            "succeeded",
            "): durable",
        ],
    );
    sink.finished();
}

#[test]
fn a_seal_with_nothing_to_flush_begins_and_ends_with_no_flush_pair() {
    let sink = Checked::pairs();
    let mut rig = plain(&sink);
    assert_eq!(rig.seal(1), DurabilityRequest::Submitted);
    rig.run();
    rig.entries();
    says(&sink.lines(), &["began", "durable"]);
    sink.finished();
}

#[test]
fn a_seal_waiting_for_its_write_begins_before_its_flush_does() {
    let sink = Checked::pairs();
    let mut rig = plain(&sink);
    rig.write(1, 0, b"x").expect("accepted");
    assert_eq!(rig.seal(1), DurabilityRequest::Submitted);
    says(&sink.lines(), &["seal through 1 (pair"]);
    rig.run();
    rig.entries();
    says(
        &sink.lines(),
        &["seal through 1", "flush of file 1", "succeeded", "durable"],
    );
    sink.finished();
}

#[test]
fn each_file_a_seal_flushes_is_its_own_flush_pair() {
    let sink = Checked::pairs();
    let mut rig = plain(&sink);
    rig.attach(super::OTHER, MemoryTarget::new());
    rig.write_to(KEY, 1, 0, b"a").expect("accepted");
    rig.write_to(super::OTHER, 1, 0, b"b").expect("accepted");
    rig.run();
    rig.seal(1);
    rig.run();
    rig.entries();
    let lines = sink.lines();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("flush of file") && l.ends_with("began"))
            .count(),
        2
    );
    assert_eq!(lines.iter().filter(|l| l.ends_with("succeeded")).count(), 2);
    sink.finished();
}

#[test]
fn a_seal_sealed_again_begins_no_second_pair() {
    let sink = Checked::pairs();
    let mut rig = plain(&sink);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.seal(1);
    assert!(matches!(rig.seal(1), DurabilityRequest::AlreadySealed(_)));
    rig.run();
    rig.entries();
    assert_eq!(
        sink.lines().iter().filter(|l| l.ends_with("began")).count(),
        2,
        "one seal, one flush"
    );
    sink.finished();
}

#[test]
fn a_failed_flush_ends_its_pair_failed_and_its_seals_failed_and_a_later_seal_ends_blocked() {
    let sink = Checked::pairs();
    let script = FaultScript::new().on(
        When::any().op(ScriptOp::Commit).nth(1),
        Fault::Fail(scripted_fault("the flush failed")),
    );
    let mut rig = rig(&sink, script, &[], &[]);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.seal(1);
    rig.run();
    let token = rig
        .entries()
        .into_iter()
        .find_map(|entry| match entry {
            crate::types::Entry::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .expect("a failure");
    says(
        &sink.lines(),
        &[
            "seal through 1",
            "flush of file 1",
            "failed",
            "seal through 1 (pair",
        ],
    );
    assert!(sink.lines()[3].ends_with("failed"), "{:?}", sink.lines());

    rig.write(2, 0, b"y").expect("accepted");
    rig.run();
    rig.seal(2);
    rig.run();
    rig.entries();
    let lines = sink.lines();
    assert!(
        lines
            .last()
            .is_some_and(|l| l.contains("seal through 2") && l.ends_with("blocked")),
        "{lines:#?}"
    );

    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("ours");
    rig.act(actions);
    rig.run();
    rig.entries();
    assert_eq!(
        sink.lines().len(),
        lines.len(),
        "the seals answered already end no more"
    );
    sink.finished();
}

#[test]
fn a_provider_domain_is_a_flush_pair_that_ends_as_the_provider_answers() {
    for (outcome, ends) in [
        (DomainOutcome::Succeeded, "succeeded"),
        (
            DomainOutcome::Failed(crate::report_io_error(std::io::Error::other("no"))),
            "failed",
        ),
        (DomainOutcome::Abandoned, "abandoned"),
    ] {
        let sink = Checked::pairs();
        let mut rig = rig(&sink, FaultScript::new(), &["log"], &["log"]);
        rig.write(1, 0, b"x").expect("accepted");
        rig.run();
        rig.seal(1);
        rig.run();
        let request = rig.requests.pop().expect("a request");
        let actions = rig
            .core
            .provided(request.lineage, request.seal, &domain("log"), outcome);
        rig.act(actions);
        rig.run();
        rig.entries();
        let lines = sink.lines();
        let flush: Vec<&String> = lines
            .iter()
            .filter(|l| l.contains("flush of domain \"log\""))
            .collect();
        assert_eq!(flush.len(), 2, "{lines:#?}");
        assert!(flush[1].ends_with(ends), "{lines:#?}");
        let seal_ends = if ends == "succeeded" {
            "durable"
        } else {
            "failed"
        };
        assert!(
            lines.last().is_some_and(|l| l.ends_with(seal_ends)),
            "{lines:#?}"
        );
        sink.finished();
    }
}

#[test]
fn ending_a_lineage_ends_its_open_seal_unanswered_and_its_open_flush_abandoned() {
    let sink = Checked::pairs();
    let script = FaultScript::new().on(When::any().op(ScriptOp::Commit), Fault::Hold);
    let mut rig = rig(&sink, script, &[], &[]);
    let minted = rig.core.mint_lineage(None);
    rig.write_with(KEY, Epoch::new(minted, 1), 0, b"x", Default::default())
        .expect("accepted");
    rig.run();
    rig.seal_in(minted, 1);
    rig.run();
    says(&sink.lines(), &["seal through 1", "flush of file 1"]);
    let actions = rig.core.end_lineage(minted).expect("live, not the default");
    rig.act(actions);
    let lines = sink.lines();
    assert!(lines.iter().any(|l| l.ends_with("abandoned")), "{lines:#?}");
    assert!(
        lines.iter().any(|l| l.ends_with("ended unanswered")),
        "{lines:#?}"
    );
    rig.backend.release_all();
    rig.run();
    rig.entries();
    assert_eq!(
        sink.lines().len(),
        lines.len(),
        "the late flush ends nothing"
    );
    sink.finished();
}

#[test]
fn closing_the_core_ends_every_pair_still_open() {
    let sink = Checked::pairs();
    let script = FaultScript::new().on(When::any().op(ScriptOp::Commit), Fault::Hold);
    let mut rig = rig(&sink, script, &[], &[]);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.seal(1);
    rig.run();
    rig.write(2, 0, b"y").expect("accepted");
    rig.seal(2);
    assert_eq!(
        sink.lines().iter().filter(|l| l.ends_with("began")).count(),
        3
    );
    let Rig { core, .. } = rig;
    drop(core.close());
    let lines = sink.lines();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.ends_with("ended unanswered"))
            .count(),
        2,
        "{lines:#?}"
    );
    assert_eq!(
        lines.iter().filter(|l| l.ends_with("abandoned")).count(),
        1,
        "{lines:#?}"
    );
    sink.finished();
}

#[test]
fn dropping_the_core_ends_every_pair_still_open() {
    let sink = Checked::pairs();
    let script = FaultScript::new().on(When::any().op(ScriptOp::Commit), Fault::Hold);
    let mut rig = rig(&sink, script, &[], &[]);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.seal(1);
    rig.run();
    drop(rig);
    let lines = sink.lines();
    assert!(
        lines.iter().any(|l| l.ends_with("ended unanswered")),
        "{lines:#?}"
    );
    assert!(lines.iter().any(|l| l.ends_with("abandoned")), "{lines:#?}");
    sink.finished();
}

#[test]
fn a_sink_that_wants_neither_pair_is_told_of_none() {
    let sink = Checked::wanting(&[]);
    let mut rig = plain(&sink);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.seal(1);
    rig.run();
    rig.entries();
    assert!(sink.lines().is_empty());
    sink.finished();
}

#[test]
fn a_sink_that_wants_one_pair_is_told_of_that_one_only() {
    for (wants, about) in [
        (DiagnosticKind::Flush, "flush of"),
        (DiagnosticKind::Seal, "(pair"),
    ] {
        let sink = Checked::wanting(&[wants]);
        let mut rig = plain(&sink);
        rig.write(1, 0, b"x").expect("accepted");
        rig.run();
        rig.seal(1);
        rig.run();
        rig.entries();
        let lines = sink.lines();
        assert_eq!(lines.len(), 2, "{lines:#?}");
        assert!(lines.iter().all(|l| l.contains(about)), "{lines:#?}");
        if wants == DiagnosticKind::Seal {
            assert!(lines.iter().all(|l| !l.contains("flush of")), "{lines:#?}");
        }
        sink.finished();
    }
}

#[test]
fn many_seals_each_end_once() {
    let sink = Checked::pairs();
    let mut rig = plain(&sink);
    for epoch in 1..=20 {
        rig.write(epoch, 0, b"x").expect("accepted");
        rig.seal(epoch);
        rig.run();
    }
    rig.run();
    rig.entries();
    let lines = sink.lines();
    assert_eq!(
        lines.iter().filter(|l| l.ends_with("durable")).count(),
        20,
        "{lines:#?}"
    );
    sink.finished();
}

#[test]
fn a_seal_with_a_file_and_a_domain_is_a_flush_pair_for_each_part() {
    let sink = Checked::pairs();
    let mut rig = rig(&sink, FaultScript::new(), &["log"], &["log", "disk"]);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.seal(1);
    rig.run();
    let request = rig.requests.pop().expect("a request");
    let actions = rig.core.provided(
        request.lineage,
        request.seal,
        &domain("log"),
        DomainOutcome::Succeeded,
    );
    rig.act(actions);
    rig.run();
    rig.entries();
    let lines = sink.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("flush of file 1") && l.ends_with("succeeded")),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("flush of domain \"log\"") && l.ends_with("succeeded")),
        "{lines:#?}"
    );
    assert!(
        lines.last().is_some_and(|l| l.ends_with("durable")),
        "{lines:#?}"
    );
    sink.finished();
}

#[test]
fn a_minted_lineages_pairs_name_its_lineage() {
    let sink = Checked::pairs();
    let mut rig = plain(&sink);
    let minted = rig.core.mint_lineage(None);
    rig.write_with(KEY, Epoch::new(minted, 3), 0, b"x", Default::default())
        .expect("accepted");
    rig.run();
    rig.seal_in(minted, 3);
    rig.run();
    rig.entries();
    let lines = sink.lines();
    assert!(!lines.is_empty());
    assert!(
        lines
            .iter()
            .all(|l| l.contains(&format!("lineage {} seal through 3", minted.seq))),
        "{lines:#?}"
    );
    sink.finished();
}
