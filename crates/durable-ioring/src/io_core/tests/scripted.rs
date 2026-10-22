// Copyright (c) 2026 Mike Grier
//! The core driven by a fault script (DI-11.1): the failures it must handle, produced on demand by
//! the in-memory backend and not by detaching a target underneath it.

use super::{KEY, Rig, shown};
use crate::backend::RefusalCause;
use crate::memory::{Fault, FaultScript, MEMORY, ScriptOp, When, scripted_fault};
use crate::types::{Entry, EpochState, FailureCause, PushRefusal};

const WRITES: When = When::any().op(ScriptOp::Write);
const COMMITS: When = When::any().op(ScriptOp::Commit);

fn scripted(when: When, fault: Fault) -> Rig {
    Rig::scripted(FaultScript::new().on(when, fault))
}

#[test]
fn a_scripted_commit_failure_blocks_the_epoch_and_names_its_target() {
    let mut rig = scripted(COMMITS, Fault::Fail(scripted_fault("the flush failed")));
    rig.write(1, 0, b"lost").expect("accepted");
    rig.seal(1);
    rig.run();
    let entries = rig.entries();
    assert!(matches!(entries[0], Entry::Op(_)));
    let Entry::Failed(failed) = &entries[1] else {
        panic!("a failure: {entries:?}");
    };
    let FailureCause::Commit { target, fault } = &failed.cause else {
        panic!("a commit's failure: {:?}", failed.cause);
    };
    assert_eq!(*target, KEY);
    assert_eq!(fault.source, MEMORY);
    assert_eq!(failed.suspect.writes().len(), 1);
    assert!(
        matches!(
            rig.core.epoch_state(rig.epoch(1)),
            Ok(EpochState::Blocked(_))
        ),
        "the failure blocks the epoch"
    );
    assert!(
        rig.target().durable_contents().is_empty(),
        "the target was never detached, and nothing it holds is durable"
    );
    assert_eq!(rig.backend.fired().len(), 1);
}

#[test]
fn a_held_write_completion_keeps_the_seal_from_committing_until_it_is_released() {
    let mut rig = scripted(WRITES.nth(1), Fault::Hold);
    rig.write(1, 0, b"x").expect("accepted");
    rig.seal(1);
    rig.run();
    assert!(rig.entries().is_empty(), "the write has not completed");
    assert!(rig.target().durable_contents().is_empty());
    assert_eq!(rig.backend.held().len(), 1);

    rig.backend.release_all();
    rig.run();
    assert_eq!(shown(&mut rig), ["op0 write@1 1 write", "durable 1"]);
    assert_eq!(rig.target().durable_contents(), b"x");
}

#[test]
fn completions_out_of_order_are_reported_and_the_seal_still_waits_for_every_write() {
    let mut rig = scripted(WRITES.nth(1), Fault::Late(1));
    rig.write(1, 0, b"a").expect("accepted");
    rig.write(2, 1, b"b").expect("accepted");
    rig.seal(2);
    rig.run();
    let shown = shown(&mut rig);
    assert_eq!(
        shown.last().map(String::as_str),
        Some("durable 2"),
        "{shown:?}"
    );
    let mut ops: Vec<&str> = shown[..shown.len() - 1]
        .iter()
        .map(String::as_str)
        .collect();
    ops.sort_unstable();
    assert_eq!(ops, ["op0 write@1 1 write", "op1 write@2 1 write"]);
    assert_eq!(rig.target().durable_contents(), b"ab");
}

#[test]
fn a_scripted_refusal_comes_back_with_its_cause_and_the_next_push_is_accepted() {
    let mut rig = scripted(WRITES.nth(1), Fault::Refuse(RefusalCause::Busy));
    let refused = rig.write(1, 0, b"kept").expect_err("scripted busy");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::Busy)
    ));
    assert_eq!(refused.buffer.as_deref(), Some(b"kept".as_slice()));
    rig.write(1, 0, b"kept").expect("only the first is refused");
    rig.run();
    assert_eq!(rig.target().contents(), b"kept");
}

#[test]
fn a_write_that_fails_by_script_never_reaches_the_target() {
    let mut rig = scripted(WRITES, Fault::Fail(scripted_fault("the disk is full")));
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    assert_eq!(shown(&mut rig), ["op0 write@1 failed by memory write"]);
    assert!(rig.target().contents().is_empty());
}
