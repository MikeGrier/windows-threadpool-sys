// Copyright (c) 2026 Mike Grier
//! A failed flush, as the core answers it (DI-18.2): the tests that armed `win-durable-ioring`'s fault
//! seam only to make a flush fail, and assert nothing the Windows driver adds, moved here with their
//! names and assertions. Each flush fails by the in-memory backend's script -- the named file's first
//! commit -- rather than by the seam. Their census and the reason each is the core's are in
//! `DI-18.1`'s record in this crate's `COMPLETED-CHECKLIST.md`; the file each came from is named on it.
//!
//! Where the Windows harness supplied its own value, the rig's stands in: the contexts are the rig's,
//! and the fault's source is the script's.

use std::sync::Arc;

use super::{Core, KEY, OTHER, Rig, V, nz};
use crate::backend::{IoBackend, RegisteredRegion, SubmissionData};
use crate::diagnostics::NoDiagnostics;
use crate::ids::{FailureId, FailureToken};
use crate::io_core::{CoreActions, DurableIoCore};
use crate::io_target::IoTarget;
use crate::memory::{
    Fault, FaultScript, MemoryBackend, MemoryTarget, ScriptOp, When, scripted_fault,
};
use crate::oracle::ConformanceOracle;
use crate::test_support::{ManualClock, Ticks};
use crate::types::{
    DurabilityRequest, Entry, Epoch, EpochState, Failed, FailureCause, FlushDomain, MarkingKind,
    OpKind, PushRefusal, RemoveTargetError, Resolution, ResolveRefusal, WriteOptions,
};

const GIVEN: crate::types::IoTargetKey = KEY;
const ADDED: crate::types::IoTargetKey = OTHER;

type RigEntry = Entry<V, Vec<u8>, &'static str>;

/// A script that fails `target`'s first commit: the seam's "fail the next flush of this file", for a
/// test whose armed flush is the file's first.
fn first_flush_fails(target: crate::types::IoTargetKey) -> FaultScript {
    FaultScript::new().on(
        When::any().op(ScriptOp::Commit).target(target).nth(1),
        Fault::Fail(scripted_fault("the flush failed")),
    )
}

/// A rig whose backend fails `fails`' first flush and has `registered` registered, with `targets`
/// attached, each declared with its domains.
fn rig_with(
    fails: crate::types::IoTargetKey,
    registered: Vec<Vec<u8>>,
    targets: &[(crate::types::IoTargetKey, &[&str])],
) -> Rig {
    let mut rig = Rig::over(Core::new(Ticks::default(), Arc::new(NoDiagnostics)));
    rig.backend = MemoryBackend::new(nz(8), nz(16))
        .with_registered(registered)
        .with_script(first_flush_fails(fails));
    for (key, domains) in targets {
        rig.attach(
            *key,
            MemoryTarget::new()
                .with_domains(domains.iter().map(|d| FlushDomain::new(d.as_bytes()))),
        );
    }
    rig
}

/// A rig with `GIVEN` alone, whose first flush fails.
fn one() -> Rig {
    rig_with(GIVEN, Vec::new(), &[(GIVEN, &[])])
}

/// The next entry, after running the backend, once the oracle has accepted it.
fn next_entry(rig: &mut Rig) -> RigEntry {
    rig.run();
    let entry = rig.core.pop().expect("an entry");
    rig.oracle
        .observe(&entry)
        .expect("the oracle accepts the entry");
    entry
}

/// The next entry, which must be a completion.
fn next_op(rig: &mut Rig) {
    match next_entry(rig) {
        Entry::Op(_) => {}
        other => panic!("expected a completion, got {other:?}"),
    }
}

fn next_failed(rig: &mut Rig) -> Failed<V> {
    match next_entry(rig) {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// Whether nothing more is poppable, once the backend has run.
fn nothing_more(rig: &mut Rig) -> bool {
    rig.run();
    rig.core.pop().is_none()
}

/// The ops a failure suspects, by sequence number.
fn suspects(failed: &Failed<V>) -> Vec<u64> {
    failed.suspect.writes().iter().map(|w| w.op.seq).collect()
}

/// Resolve, reporting each heal to the oracle.
fn resolve(rig: &mut Rig, items: Vec<(FailureToken, Resolution)>) {
    let healed: Vec<FailureId> = items
        .iter()
        .filter(|(_, resolution)| *resolution == Resolution::Heal)
        .map(|(token, _)| token.id())
        .collect();
    let actions = rig.core.resolve(items).expect("resolve");
    for id in healed {
        rig.oracle.healed(id);
    }
    rig.act(actions);
}

fn state(rig: &Rig, id: u64) -> EpochState<V> {
    rig.core
        .epoch_state(rig.epoch(id))
        .expect("the default lineage is live")
}

fn finish(rig: Rig) {
    rig.oracle.finish().expect("every operation completed");
}

/// Two writes tagged 1 to `GIVEN`, completed, then sealed through 1 with its flush failing.
fn failed_seal(rig: &mut Rig) -> Failed<V> {
    rig.write_to(GIVEN, 1, 0, b"first").expect("accepted");
    rig.write_to(GIVEN, 1, 8, b"second").expect("accepted");
    next_op(rig);
    next_op(rig);
    assert_eq!(rig.seal(1), DurabilityRequest::Submitted);
    next_failed(rig)
}

/// From `win-durable-ioring`'s `dioring/tests/failures.rs`: its core half. The Windows half -- the
/// cause relayed from the ring, with its source and its `HRESULT` -- stays there.
#[test]
fn a_failed_flush_is_reported_with_its_cause_and_holds_the_mark() {
    let mut rig = one();
    let failed = failed_seal(&mut rig);
    match &failed.cause {
        FailureCause::Commit { target: file, .. } => assert_eq!(*file, GIVEN),
        other => panic!("expected a flush failure, got {other:?}"),
    }
    assert_eq!(suspects(&failed), [0, 1]);
    assert_eq!(
        rig.core.durable_through(rig.core.default_lineage()),
        Ok(None)
    );
    assert_eq!(state(&rig, 1), EpochState::Blocked(failed.id));
    assert_eq!(
        rig.seal(1),
        DurabilityRequest::AlreadySealed(EpochState::Blocked(failed.id))
    );
    let inventory = rig.core.failures();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].id, failed.id);
    assert!(inventory[0].token_live);
    assert!(nothing_more(&mut rig), "one answer");
    drop(failed);
    finish(rig);
}

/// From `failures.rs`: CONTRACT.md's "Healing a failure", over the core.
#[test]
fn a_healed_failure_is_passed_at_the_first_seal_after_the_heal() {
    let mut rig = one();
    let failed = failed_seal(&mut rig);
    let refused = rig.write_to(GIVEN, 1, 0, &[1]).expect_err("1 is sealed");
    assert_eq!(refused.context, "write");

    rig.write_to(GIVEN, 2, 0, b"FIRST").expect("accepted");
    rig.write_to(GIVEN, 2, 8, b"SECOND").expect("accepted");
    next_op(&mut rig);
    next_op(&mut rig);
    resolve(&mut rig, vec![(failed.token, Resolution::Heal)]);
    assert!(
        rig.core.take_token(failed.id).is_none(),
        "the heal holds it"
    );
    assert_eq!(rig.seal(2), DurabilityRequest::Submitted);
    match next_entry(&mut rig) {
        Entry::Healed {
            failure,
            suspect,
            markings,
        } => {
            assert_eq!(failure, failed.id);
            assert_eq!(suspect.writes(), failed.suspect.writes());
            assert!(markings.is_empty(), "nothing happened to 1's writes after");
        }
        other => panic!("expected Healed before any Durable, got {other:?}"),
    }
    for id in [1, 2] {
        match next_entry(&mut rig) {
            Entry::Durable { through } => assert_eq!(through, rig.epoch(id)),
            other => panic!("expected Durable through {id}, got {other:?}"),
        }
    }
    assert!(rig.core.failures().is_empty());
    assert_eq!(state(&rig, 1), EpochState::Durable);
    finish(rig);
}

/// From `failures.rs`.
#[test]
fn without_the_heal_a_later_seal_is_answered_blocked() {
    let mut rig = one();
    let failed = failed_seal(&mut rig);
    rig.write_to(GIVEN, 2, 0, b"again").expect("accepted");
    next_op(&mut rig);
    assert_eq!(rig.seal(2), DurabilityRequest::Submitted);
    match next_entry(&mut rig) {
        Entry::Blocked { through, by } => {
            assert_eq!((through, by), (rig.epoch(2), failed.id));
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    assert_eq!(state(&rig, 2), EpochState::Blocked(failed.id));
    resolve(&mut rig, vec![(failed.token, Resolution::Abandon)]);
    match next_entry(&mut rig) {
        Entry::Abandoned {
            failure, suspect, ..
        } => {
            assert_eq!(failure, failed.id);
            assert_eq!(suspect.writes().len(), 2);
        }
        other => panic!("expected Abandoned, got {other:?}"),
    }
    let mut durable = Vec::new();
    for _ in 0..2 {
        match next_entry(&mut rig) {
            Entry::Durable { through } => durable.push(through),
            other => panic!("expected Durable, got {other:?}"),
        }
    }
    assert_eq!(durable, [rig.epoch(1), rig.epoch(2)]);
    assert_eq!(state(&rig, 1), EpochState::Abandoned);
    assert_eq!(state(&rig, 2), EpochState::Durable);
    finish(rig);
}

/// From `failures.rs`.
#[test]
fn a_failed_flush_reaches_only_files_sharing_a_declared_domain() {
    let mut rig = rig_with(
        ADDED,
        Vec::new(),
        &[(GIVEN, &["disk-1"]), (ADDED, &["disk-2"])],
    );
    rig.write_to(GIVEN, 1, 0, b"log").expect("accepted");
    rig.write_to(ADDED, 1, 0, b"data").expect("accepted");
    next_op(&mut rig);
    next_op(&mut rig);
    rig.seal(1);
    let failed = next_failed(&mut rig);
    assert_eq!(suspects(&failed), [1], "the log's disk is not the data's");
    drop(failed);
    finish(rig);
}

/// From `failures.rs`.
#[test]
fn a_resolution_naming_another_instances_failure_is_refused_whole() {
    let mut rig = one();
    let ours = failed_seal(&mut rig);
    let mut other = Rig::new();
    let (_, actions) = other
        .core
        .import_failure(crate::types::ImportScope::All)
        .expect("import");
    other.act(actions);
    let theirs = match other.core.pop() {
        Some(Entry::Failed(failed)) => failed,
        other => panic!("expected Failed, got {other:?}"),
    };
    let error = rig
        .core
        .resolve(vec![
            (ours.token, Resolution::Abandon),
            (theirs.token, Resolution::Abandon),
        ])
        .expect_err("a foreign token");
    assert!(matches!(error.reason, ResolveRefusal::Foreign(id) if id == theirs.id));
    assert_eq!(error.returned.len(), 2);
    assert!(nothing_more(&mut rig), "nothing was abandoned");
    assert_eq!(rig.core.failures().len(), 1);
    finish(rig);
}

/// From `failures.rs`.
#[test]
fn a_closed_failure_stays_and_its_token_can_be_taken_again() {
    let mut rig = one();
    let failed = failed_seal(&mut rig);
    assert!(rig.core.take_token(failed.id).is_none(), "live");
    failed.token.close();
    assert!(!rig.core.failures()[0].token_live);
    let token = rig.core.take_token(failed.id).expect("closed");
    resolve(&mut rig, vec![(token, Resolution::Abandon)]);
    assert!(matches!(next_entry(&mut rig), Entry::Abandoned { .. }));
    assert!(matches!(next_entry(&mut rig), Entry::Durable { .. }));
    assert!(
        rig.core.take_token(failed.id).is_none(),
        "resolved, and leaves no memory"
    );
    finish(rig);
}

/// From `failures.rs`.
#[test]
fn a_write_into_an_abandoned_epoch_is_refused_with_what_it_took() {
    let mut rig = rig_with(GIVEN, vec![vec![0; 16]], &[(GIVEN, &[])]);
    rig.write_to(GIVEN, 1, 0, b"sealed").expect("accepted");
    rig.write_to(GIVEN, 3, 8, b"open").expect("accepted");
    next_op(&mut rig);
    next_op(&mut rig);
    rig.seal(1);
    let failed = next_failed(&mut rig);
    assert_eq!(suspects(&failed), [0, 1], "sealed 1 and open 3");
    resolve(&mut rig, vec![(failed.token, Resolution::Abandon)]);
    assert!(matches!(next_entry(&mut rig), Entry::Abandoned { .. }));
    assert!(matches!(next_entry(&mut rig), Entry::Durable { .. }));

    let three = rig.epoch(3);
    let error = rig
        .write_to(GIVEN, 3, 0, &[7])
        .expect_err("3 was abandoned, though it is open");
    assert!(matches!(error.reason, PushRefusal::EpochAbandoned { epoch } if epoch == three));
    assert_eq!((error.buffer, error.context), (Some(vec![7]), "write"));
    let region = RegisteredRegion {
        index: 0,
        offset: 0,
        length: 4,
    };
    let error = rig
        .core
        .write(
            GIVEN,
            0,
            SubmissionData::Registered(region),
            three,
            "span",
            WriteOptions::new(),
            &mut rig.backend,
        )
        .expect_err("a registered write to 3");
    assert!(matches!(error.reason, PushRefusal::EpochAbandoned { .. }));
    assert_eq!((error.buffer, error.context), (None, "span"));
    let error = rig
        .write_to(GIVEN, 1, 0, &[7])
        .expect_err("1 is sealed and abandoned");
    assert!(
        matches!(error.reason, PushRefusal::Sealed { .. }),
        "the seal is checked first: {:?}",
        error.reason
    );
    assert!(nothing_more(&mut rig), "a refused write completes nothing");

    rig.write_to(GIVEN, 4, 0, b"later").expect("accepted");
    next_op(&mut rig);
    finish(rig);
}

/// From `dioring/tests/ending.rs`.
#[test]
fn a_failure_suspecting_a_write_holds_its_file_until_it_is_resolved() {
    let mut rig = one();
    rig.write_to(GIVEN, 1, 0, b"one").expect("accepted");
    next_op(&mut rig);
    assert_eq!(rig.seal(1), DurabilityRequest::Submitted);
    let failed = next_failed(&mut rig);

    let refused = match rig.core.remove_target(GIVEN) {
        Err(RemoveTargetError::Busy(busy)) => busy,
        Err(other) => panic!("expected Busy, got {other:?}"),
        Ok(_) => panic!("expected Busy, and the file was removed"),
    };
    assert_eq!(refused.failures, [failed.id]);
    assert_eq!(
        refused.uncovered, 0,
        "its suspect write is reported by the failure"
    );
    assert!(refused.to_string().contains("heal or abandon"), "{refused}");

    resolve(&mut rig, vec![(failed.token, Resolution::Abandon)]);
    assert!(matches!(next_entry(&mut rig), Entry::Abandoned { .. }));
    rig.core.remove_target(GIVEN).expect("resolved");
    finish(rig);
}

/// From `dioring/tests/ending.rs`: what the core's `close` hands back, which the driver's hands on.
#[test]
fn closing_hands_back_the_unresolved_failures() {
    let mut rig = one();
    rig.write_to(GIVEN, 1, 0, b"one").expect("accepted");
    next_op(&mut rig);
    rig.seal(1);
    let failed = next_failed(&mut rig);
    let left = rig.core.close();
    let ids: Vec<_> = left.failures.iter().map(|f| f.id).collect();
    assert_eq!(ids, [failed.id]);
    assert_eq!(left.lineages[0].durable_through, None);
}

/// From `dioring/tests/lineages.rs`: a failure suspecting a held write of each lineage holds both,
/// and a heal takes effect in each at its own next seal (DI-D-41).
#[test]
fn a_failure_spanning_two_lineages_heals_in_each_at_its_own_seal() {
    let mut rig = rig_with(GIVEN, Vec::new(), &[(GIVEN, &[]), (ADDED, &[])]);
    let minted = rig.core.mint_lineage(Some("log".to_owned()));
    let own = rig.write_to(GIVEN, 1, 0, b"own").expect("accepted");
    let theirs = rig
        .write_with(
            ADDED,
            Epoch::new(minted, 1),
            0,
            b"theirs",
            WriteOptions::new(),
        )
        .expect("accepted");
    next_op(&mut rig);
    next_op(&mut rig);

    rig.seal(1);
    let failed = next_failed(&mut rig);
    let suspects: Vec<_> = failed.suspect.writes().iter().map(|w| w.op).collect();
    assert_eq!(suspects, [own, theirs], "a held write of each lineage");
    resolve(&mut rig, vec![(failed.token, Resolution::Heal)]);

    assert_eq!(rig.seal(2), DurabilityRequest::Submitted);
    for id in [1, 2] {
        match next_entry(&mut rig) {
            Entry::Durable { through } => assert_eq!(through, rig.epoch(id)),
            other => panic!("expected Durable through {id}, got {other:?}"),
        }
    }
    assert_eq!(rig.core.failures().len(), 1, "it still holds the other");
    assert_eq!(
        rig.core.epoch_state(Epoch::new(minted, 1)),
        Ok(EpochState::Open)
    );

    assert_eq!(rig.seal_in(minted, 1), DurabilityRequest::Submitted);
    loop {
        match next_entry(&mut rig) {
            Entry::Marked { .. } => continue,
            Entry::Healed { failure, .. } => {
                assert_eq!(failure, failed.id);
                break;
            }
            other => panic!("expected Healed, got {other:?}"),
        }
    }
    match next_entry(&mut rig) {
        Entry::Durable { through } => assert_eq!(through, Epoch::new(minted, 1)),
        other => panic!("expected Durable through the minted lineage's 1, got {other:?}"),
    }
    assert!(rig.core.failures().is_empty());
    finish(rig);
}

/// From `dioring/tests/markings.rs`.
#[test]
fn a_heal_reports_its_record_with_the_markings_its_failure_gained() {
    let mut rig = one();
    let sealed = rig.write_to(GIVEN, 1, 0, b"first").expect("accepted");
    let later = rig.write_to(GIVEN, 2, 8, b"second").expect("accepted");
    next_op(&mut rig);
    next_op(&mut rig);
    assert_eq!(rig.seal(1), DurabilityRequest::Submitted);
    let failed = next_failed(&mut rig);
    let suspects: Vec<_> = failed.suspect.writes().iter().map(|w| w.op).collect();
    assert_eq!(suspects, [sealed, later]);

    resolve(&mut rig, vec![(failed.token, Resolution::Heal)]);
    assert_eq!(rig.seal(2), DurabilityRequest::Submitted);
    let covered = match next_entry(&mut rig) {
        Entry::Marked { marking, .. } => marking,
        other => panic!("expected Marked, got {other:?}"),
    };
    assert_eq!(
        (covered.write, covered.kind.clone()),
        (
            later,
            MarkingKind::Covered {
                flushed: true,
                provider: false
            }
        ),
        "a healing failure goes on gaining markings"
    );
    match next_entry(&mut rig) {
        Entry::Healed {
            failure, markings, ..
        } => {
            assert_eq!(failure, failed.id);
            assert_eq!(markings, [covered]);
        }
        other => panic!("expected Healed, got {other:?}"),
    }
    for id in [1, 2] {
        match next_entry(&mut rig) {
            Entry::Durable { through } => assert_eq!(through, rig.epoch(id)),
            other => panic!("expected Durable through {id}, got {other:?}"),
        }
    }
    finish(rig);
}

/// A core over a clock the test moves, with its backend and the oracle: the rig's part played by
/// hand, since the rig's core counts its own readings.
struct Clocked {
    core: DurableIoCore<u64, Vec<u8>, &'static str, ManualClock>,
    backend: MemoryBackend,
    oracle: ConformanceOracle<V, &'static str>,
}

impl Clocked {
    fn act(&mut self, actions: CoreActions<V, Vec<u8>>) {
        let mut pending = std::collections::VecDeque::from([actions]);
        while let Some(actions) = pending.pop_front() {
            for submission in actions.submit {
                if let Err(refused) = self.backend.push(submission) {
                    pending.push_back(self.core.refused(refused));
                }
            }
        }
    }

    fn next_entry(&mut self) -> RigEntry {
        loop {
            self.backend
                .submit()
                .expect("the memory backend's submit never fails");
            let completions: Vec<_> =
                std::iter::from_fn(|| self.backend.pop().expect("never fails")).collect();
            for completion in completions {
                let actions = self.core.complete(completion);
                self.act(actions);
            }
            if let Some(entry) = self.core.pop() {
                self.oracle
                    .observe(&entry)
                    .expect("the oracle accepts the entry");
                return entry;
            }
            assert!(self.backend.queued() > 0, "no entry, and nothing to run");
        }
    }
}

/// From `dioring/tests/stamps.rs`.
#[test]
fn an_instance_given_a_clock_stamps_a_failed_flush_with_its_reading_when_observed() {
    let clock = ManualClock::at(7_000);
    let mut rig = Clocked {
        core: DurableIoCore::new(clock.clone(), Arc::new(NoDiagnostics)),
        backend: MemoryBackend::new(nz(8), nz(16)).with_script(first_flush_fails(GIVEN)),
        oracle: ConformanceOracle::new(),
    };
    let target = MemoryTarget::new();
    rig.core
        .add_target(GIVEN, target.describe())
        .expect("a new key");
    rig.backend.attach(GIVEN, target).expect("a new key");
    let epoch = Epoch::new(rig.core.default_lineage(), 1);
    let id = rig
        .core
        .write(
            GIVEN,
            0,
            SubmissionData::Owned(b"stamped".to_vec()),
            epoch,
            "write",
            WriteOptions::new(),
            &mut rig.backend,
        )
        .expect("accepted");
    rig.oracle
        .pushed(id, OpKind::Write { epoch }, 7, "write")
        .expect("the oracle accepts the push");
    assert!(matches!(rig.next_entry(), Entry::Op(_)));
    clock.advance(3_000);
    let (answer, actions) = rig
        .core
        .make_durable_through(epoch)
        .expect("a live lineage");
    assert_eq!(answer, DurabilityRequest::Submitted);
    rig.oracle.sealed(epoch);
    rig.act(actions);
    let failed = match rig.next_entry() {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    };
    assert_eq!(failed.observed, 10_000);
    let inventory = rig.core.failures();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].observed, failed.observed);
    rig.oracle.finish().expect("every operation completed");
}
