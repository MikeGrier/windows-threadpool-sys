// Copyright (c) 2026 Mike Grier
//! Ending (DI-3.2.7, DI-D-25, DI-D-45), against real files: removing a file, refused with every
//! hold reported at once; and ending an instance, `close()` handing back what is left, and both it
//! and drop waiting for the consumer's provider. Also an inconsistency the hub finds, which it
//! panics on where it finds it (DI-D-46).

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use windows_ioring_sys::RegisteredSpan;

use super::gates::{expect_durable, read_gated, two, write_gated};
use super::pushes::{ADDED, BOUND, GIVEN, Harness, Ring, given, instance, temp};
use super::{FlushDomain, FlushRequest, V, domain, empty, file, keeper, setup};
use crate::windows_backend::Win32FileTarget;
use durable_ioring::backend::SubmissionData;
use durable_ioring::driver::{Answer, DomainCall, Home};
use durable_ioring::{DomainOutcome, DurabilityProvider};
use durable_ioring::{
    DurabilityRequest, Entry, Epoch, Failed, IoTargetBusy, IoTargetKey, OpKind, PushRefusal,
    ReadOptions, RemoveTargetError, Resolution, WriteOptions,
};
use durable_ioring::{DurableRing, RegisteredBufferRing};

/// `ERROR_IO_DEVICE`, the failure the seam is armed with.
const IO_DEVICE: u32 = 1117;

/// `key`'s refusal, which must be `Busy`.
fn busy<K: crate::dioring::TimeBase>(ring: &mut Ring<K>, key: IoTargetKey) -> IoTargetBusy<V> {
    match ring.remove_target(key) {
        Err(RemoveTargetError::Busy(busy)) => busy,
        Err(other) => panic!("expected Busy, got {other:?}"),
        Ok(_) => panic!("expected Busy, and the file was removed"),
    }
}

fn next_failed(harness: &mut Harness) -> Failed<V> {
    match harness.next_entry() {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// The message a caught panic carried.
fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

#[test]
fn removing_a_file_never_given_is_refused_as_unknown() {
    let mut ring = instance(Vec::new(), Vec::new());
    assert!(matches!(
        ring.remove_target(ADDED),
        Err(RemoveTargetError::UnknownKey(ADDED))
    ));
}

#[test]
fn an_idle_file_added_later_comes_back_and_its_key_can_be_reused() {
    let temp = temp(&[0; 16]);
    let mut ring = instance(Vec::new(), Vec::new());
    let handle = temp.open();
    ring.add_target(ADDED, Win32FileTarget::new(handle.clone()))
        .expect("add a file");
    let back = ring.remove_target(ADDED).expect("remove it");
    assert!(
        back.handle().same_handle(handle.handle()),
        "the consumer's own target"
    );

    let refused = ring
        .read(ADDED, 0, vec![0; 4], 1)
        .expect_err("the file is gone");
    assert!(matches!(refused.reason, PushRefusal::UnknownFile(ADDED)));
    assert!(matches!(
        ring.remove_target(ADDED),
        Err(RemoveTargetError::UnknownKey(ADDED))
    ));
    ring.add_target(ADDED, back).expect("the key is free again");
}

#[test]
fn a_file_given_at_construction_comes_back_and_stays_open_until_the_instance_ends() {
    let temp = temp(&[0; 16]);
    let setup = given(GIVEN, &temp);
    let original = setup.target.handle().clone();
    let mut ring = instance(vec![setup], Vec::new());
    let back = ring.remove_target(GIVEN).expect("remove it");
    assert!(back.handle().same_handle(&original));
    drop((back, original));
    assert!(temp.is_held(), "the ring's registration still holds it");
    ring.add_target(GIVEN, Win32FileTarget::new(temp.open()))
        .expect("the key is free again");
    drop(ring);
    assert!(!temp.is_held(), "released with the instance");
}

#[test]
fn an_uncovered_write_holds_its_file_until_it_is_sealed() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    harness.write(GIVEN, 0, b"one", 1, 1);
    harness.write(GIVEN, 8, b"two", 3, 2);
    harness.next_n(2);

    let refused = busy(&mut harness.ring, GIVEN);
    assert_eq!(refused.target, GIVEN);
    assert_eq!(
        (refused.in_flight, refused.held_for_gate, refused.uncovered),
        (0, 0, 2)
    );
    assert_eq!(refused.uncovered_through, [harness.epoch(3)]);
    assert!(refused.lowest_gates.is_empty() && refused.failures.is_empty());
    let text = refused.to_string();
    assert!(text.contains("2 write(s) not yet covered"), "{text}");

    assert_eq!(harness.seal(3), DurabilityRequest::Submitted);
    let three = harness.epoch(3);
    expect_durable(&mut harness, three);
    harness.ring.remove_target(GIVEN).expect("covered");
    harness.finish();
}

#[test]
fn another_files_holds_do_not_hold_this_one() {
    let (busy_file, idle_file) = (temp(&[0; 16]), temp(&[0; 16]));
    let mut harness = Harness::new(instance(
        vec![given(GIVEN, &busy_file), given(ADDED, &idle_file)],
        Vec::new(),
    ));
    harness.write(GIVEN, 0, b"held", 1, 1);
    harness.next();
    harness.ring.remove_target(ADDED).expect("nothing holds it");
    assert_eq!(busy(&mut harness.ring, GIVEN).uncovered, 1);
    harness.seal(1);
    let one = harness.epoch(1);
    expect_durable(&mut harness, one);
    harness.finish();
}

#[test]
fn held_operations_hold_their_file_with_the_lowest_gate_in_each_lineage() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let default = harness.default.clone();
    let five = harness.epoch(5);
    let three = harness.epoch(3);
    let minted_four = Epoch::new(minted.lineage(), 4);
    write_gated(&mut harness, &minted, 0, 1, 1, five);
    read_gated(&mut harness, 2, three);
    write_gated(&mut harness, &default, 8, 9, 3, minted_four);

    let refused = busy(&mut harness.ring, GIVEN);
    assert_eq!((refused.held_for_gate, refused.in_flight), (3, 0));
    assert_eq!(
        refused.lowest_gates,
        [three, minted_four],
        "the lowest in each lineage gated on, in lineage order"
    );
    assert_eq!(refused.uncovered, 0, "a held write is not uncovered");
    assert!(refused.to_string().contains("3 held until"), "{refused}");
    let left = harness.ring.close();
    assert_eq!(left.held.len(), 3);
}

#[test]
fn a_failure_suspecting_a_write_holds_its_file_until_it_is_resolved() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    harness.write(GIVEN, 0, b"one", 1, 1);
    harness.next();
    harness.ring.fail_next_flush(GIVEN, IO_DEVICE);
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let failed = next_failed(&mut harness);

    let refused = busy(&mut harness.ring, GIVEN);
    assert_eq!(refused.failures, [failed.id]);
    assert_eq!(
        refused.uncovered, 0,
        "its suspect write is reported by the failure"
    );
    assert!(refused.to_string().contains("heal or abandon"), "{refused}");

    harness
        .ring
        .resolve(vec![(failed.token, Resolution::Abandon)])
        .expect("abandon");
    assert!(matches!(harness.next_entry(), Entry::Abandoned { .. }));
    harness.ring.remove_target(GIVEN).expect("resolved");
    harness.finish();
}

/// An operation is held in flight on demand by holding the engine's lock: no pool drain can record
/// its completion until the lock is released.
#[test]
fn operations_in_flight_are_reported_and_cleared_by_their_completions() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    for i in 0..8 {
        harness.read(GIVEN, 0, 4, i);
    }
    harness.next_n(8);
    harness
        .ring
        .remove_target(GIVEN)
        .expect("every completion recorded clears its push, so nothing holds it");

    harness
        .ring
        .add_target(GIVEN, Win32FileTarget::new(temp.open()))
        .expect("add again");
    let held = {
        let mut engine = harness.ring.engine();
        let mut held = Vec::new();
        for context in [8, 9] {
            let id = engine
                .read(
                    GIVEN,
                    0,
                    SubmissionData::Owned(vec![0; 4]),
                    context,
                    ReadOptions::new(),
                )
                .expect("accepted");
            held.push((id, context));
        }
        let Err(RemoveTargetError::Busy(refused)) = engine.core.remove_target(GIVEN) else {
            panic!("two reads are in flight");
        };
        assert_eq!(refused.in_flight, 2);
        assert!(
            refused.to_string().contains("2 operation(s) in flight"),
            "{refused}"
        );
        held
    };
    for (id, context) in held {
        harness
            .oracle
            .pushed(id, OpKind::Read, 4, context)
            .expect("the oracle accepts the push");
    }
    harness.next_n(2);
    harness.ring.remove_target(GIVEN).expect("cleared again");
    harness.finish();
}

#[test]
fn closing_an_idle_instance_hands_back_its_live_lineages_with_their_marks() {
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    let two = harness.epoch(2);
    expect_durable(&mut harness, two);
    let log = harness.ring.mint_lineage(Some("log".to_owned()));
    let gone = harness.ring.mint_lineage(None);
    harness.ring.end_lineage(gone).expect("end it");
    let default = harness.lineage();

    let left = harness.ring.close();
    assert!(left.held.is_empty() && left.failures.is_empty());
    let lineages: Vec<_> = left
        .lineages
        .iter()
        .map(|info| {
            (
                info.lineage,
                info.durable_through,
                info.description.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        lineages,
        [(default, Some(2), None), (log.lineage(), None, Some("log"))],
        "the ended lineage is not live"
    );
}

#[test]
fn closing_hands_back_held_operations_in_push_order_with_what_they_took() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let gate = harness.epoch(4);
    let write = write_gated(&mut harness, &minted, 0, 1, 1, gate);
    let read = read_gated(&mut harness, 2, gate);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let registered = harness
        .ring
        .write_registered_with(
            GIVEN,
            16,
            span,
            minted.at(2),
            3,
            WriteOptions::new().gate(gate),
        )
        .expect("push a gated registered write");

    let left = harness.ring.close();
    let held: Vec<_> = left
        .held
        .iter()
        .map(|op| (op.id, op.kind, op.gate, op.buffer.clone(), op.context))
        .collect();
    let in_minted = |id| OpKind::Write {
        epoch: Epoch::new(minted.lineage(), id),
    };
    assert_eq!(
        held,
        [
            (write, in_minted(1), gate, Some(vec![1; 8]), 1),
            (read, OpKind::Read, gate, Some(vec![0; 8]), 2),
            (registered, in_minted(2), gate, None, 3),
        ]
    );
}

#[test]
fn closing_hands_back_the_unresolved_failures() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    harness.write(GIVEN, 0, b"one", 1, 1);
    harness.next();
    harness.ring.fail_next_flush(GIVEN, IO_DEVICE);
    harness.seal(1);
    let failed = next_failed(&mut harness);
    let left = harness.ring.close();
    let ids: Vec<_> = left.failures.iter().map(|f| f.id).collect();
    assert_eq!(ids, [failed.id]);
    assert_eq!(left.lineages[0].durable_through, None);
}

/// End an instance whose provider holds an unanswered request, answering it from another thread
/// once `delay` has passed; `end` ends the instance. Returns whether the answer had been given by
/// the time `end` returned.
fn end_waiting_for_an_answer(end: impl FnOnce(Harness)) -> bool {
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let ring = Ring::new(setup(
        vec![file(GIVEN.0, &temp, &["log"])],
        Vec::new(),
        provider,
    ))
    .expect("build an instance with a provider")
    .0;
    let mut harness = Harness::new(ring);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let request = kept.lock().expect("the requests").remove(0);

    let answered = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&answered);
    let answerer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        flag.store(true, Ordering::SeqCst);
        for part in request.domains {
            part.completion.succeed();
        }
    });
    end(harness);
    let in_time = answered.load(Ordering::SeqCst);
    answerer.join().expect("the answering thread");
    in_time
}

#[test]
fn close_waits_for_a_provider_completion_answered_on_another_thread() {
    assert!(end_waiting_for_an_answer(|harness| {
        let left = harness.ring.close();
        assert_eq!(
            left.lineages[0].durable_through, None,
            "taken when the instance closed, before the answer"
        );
    }));
}

#[test]
fn drop_waits_for_a_provider_completion_answered_on_another_thread() {
    assert!(end_waiting_for_an_answer(drop));
}

/// A provider that keeps every request it is handed in itself, unanswered.
#[derive(Debug)]
struct Hoards {
    domains: Vec<FlushDomain>,
    kept: Vec<FlushRequest<V>>,
}

impl DurabilityProvider<V> for Hoards {
    fn domains(&self) -> Vec<FlushDomain> {
        self.domains.clone()
    }

    fn make_durable(&mut self, request: FlushRequest<V>) {
        self.kept.push(request);
    }
}

#[test]
fn ending_does_not_wait_for_completions_the_provider_keeps_in_itself() {
    let temp = temp(&[0; 16]);
    let provider = Hoards {
        domains: vec![domain("log")],
        kept: Vec::new(),
    };
    let ring = Ring::new(setup(
        vec![file(GIVEN.0, &temp, &["log"])],
        Vec::new(),
        Some(Box::new(provider)),
    ))
    .expect("build an instance with a provider")
    .0;
    let mut harness = Harness::new(ring);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);

    let (closed, done) = mpsc::channel();
    std::thread::spawn(move || {
        let left = harness.ring.close();
        let _ = closed.send(left.lineages.len());
    });
    assert_eq!(
        done.recv_timeout(BOUND),
        Ok(1),
        "dropping the provider answers what it kept"
    );
}

#[test]
fn an_answer_for_a_completion_never_made_panics_naming_it() {
    let ring = empty();
    let call = DomainCall {
        lineage: ring.default_lineage().lineage(),
        seal: 7,
        name: domain("log"),
    };
    let found = panic::catch_unwind(AssertUnwindSafe(|| {
        Answer::answer(&*ring.hub, &call, DomainOutcome::Succeeded);
    }));
    let text = message(
        found
            .expect_err("the hub panics where it finds it")
            .as_ref(),
    );
    assert!(
        text.contains("a provider answered a completion never made")
            && text.contains("seal 7")
            && text.contains("DI-D-46"),
        "{text}"
    );
    drop(ring);
}

/// A lineage minted on `ring` and ended through its last handle, so that it is no longer live.
fn ended_lineage(ring: &mut super::Ring) -> durable_ioring::Lineage {
    let handle = ring.mint_lineage(None);
    let lineage = handle.lineage();
    ring.end_lineage(handle)
        .expect("end the minted lineage through its last handle");
    lineage
}

#[test]
fn a_release_for_a_lineage_that_is_not_live_panics_naming_it() {
    let mut ring = empty();
    let lineage = ended_lineage(&mut ring);
    let found = panic::catch_unwind(AssertUnwindSafe(|| {
        Home::release(&*ring.hub, lineage);
    }));
    let text = message(
        found
            .expect_err("the hub panics where it finds it")
            .as_ref(),
    );
    assert!(
        text.contains("the lineage was not live")
            && text.contains("Foreign")
            && text.contains("DI-D-46"),
        "{text}"
    );
    // Reported with the engine's lock released: the instance is still usable.
    assert_eq!(ring.lineages().len(), 1);
    drop(ring);
}

#[test]
fn a_release_for_the_default_lineage_is_ignored() {
    let ring = empty();
    Home::release(&*ring.hub, ring.default_lineage().lineage());
    assert_eq!(ring.lineages().len(), 1);
    drop(ring);
}
