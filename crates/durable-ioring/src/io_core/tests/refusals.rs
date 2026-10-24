// Copyright (c) 2026 Mike Grier
//! Refusals for good (DI-D-54): an operation the core would hold is put to the backend's `check`
//! first, after the core's own checks; and a submission of the core's own that the backend refuses
//! for a cause that is not retryable ends, as a completion with a `REFUSED` fault, rather than
//! being emitted again.

use super::{KEY, OTHER, Rig, V, shown};
use crate::backend::{IoBackend, REFUSED, RefusalCause, RegisteredRegion, SubmissionData};
use crate::io_target::Geometry;
use crate::memory::MemoryTarget;
use crate::types::{
    Entry, Epoch, EpochState, FailureCause, IoFault, OpKind, Outcome, PushRefusal, ReadOptions,
    WriteOptions,
};

fn gated(gate: Epoch<V>) -> WriteOptions<V> {
    WriteOptions::new().gate(gate)
}

const REGION: RegisteredRegion = RegisteredRegion {
    index: 0,
    offset: 0,
    length: 4,
};

/// The cause a `REFUSED` fault carries, checked to be one.
fn refusal(fault: &IoFault) -> &RefusalCause {
    assert_eq!(fault.source, REFUSED);
    fault
        .error
        .downcast_ref::<RefusalCause>()
        .expect("a REFUSED fault carries its cause")
}

#[test]
fn a_gated_operation_the_backend_cannot_perform_is_refused_at_push() {
    // The rig's backend has no registered buffers, so a region is one it cannot perform at all.
    let mut rig = Rig::new();
    let gate = rig.epoch(1);
    let epoch = rig.epoch(2);
    let refused = rig
        .core
        .write(
            KEY,
            0,
            SubmissionData::Registered(REGION),
            epoch,
            "gated span",
            gated(gate),
            &mut rig.backend,
        )
        .expect_err("refused at push, not held");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::Unsupported)
    ));
    assert_eq!((refused.buffer, refused.context), (None, "gated span"));

    let refused = rig
        .core
        .read(
            KEY,
            0,
            SubmissionData::Registered(REGION),
            "gated read",
            ReadOptions::new().gate(gate),
            &mut rig.backend,
        )
        .expect_err("refused at push, not held");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::Unsupported)
    ));

    // Nothing was held and no id was spent: the next accepted operation takes the first.
    let first = rig
        .read_with(0, 1, ReadOptions::new().gate(gate))
        .expect("an owned read is held");
    assert_eq!(first.seq, 0);
    let held = rig.core.close().held;
    assert_eq!(held.len(), 1, "only the owned read");
    assert_eq!(held[0].id, first);
}

#[test]
fn a_gated_operation_its_target_does_not_admit_is_refused_and_one_it_admits_is_held() {
    let mut rig = Rig::new();
    let geometry = Geometry::new(512, 512, 1).expect("powers of two");
    rig.attach(OTHER, MemoryTarget::new().with_geometry(geometry));
    let gate = rig.epoch(1);
    let epoch = rig.epoch(2);
    for (offset, length) in [(0, 3), (3, 512), (512, 511)] {
        let refused = rig
            .write_with(OTHER, epoch, offset, &vec![7; length], gated(gate))
            .expect_err("a geometry the target does not admit");
        assert!(matches!(
            refused.reason,
            PushRefusal::Backend(RefusalCause::Invalid)
        ));
        assert_eq!(refused.buffer, Some(vec![7; length]), "handed back");
    }
    let held = rig
        .write_with(OTHER, epoch, 512, &[7; 512], gated(gate))
        .expect("an aligned write is held");
    assert_eq!(held.seq, 0, "the refused ones spent no id");
    assert_eq!(rig.backend.queued(), 0, "held, not pushed");
}

#[test]
fn a_gated_operation_its_targets_access_does_not_cover_is_refused_at_push() {
    // DI-D-66: the target states what it can do, and the backend refuses what that does not cover,
    // for a gated operation at the call that gave it and not when its gate releases.
    let mut rig = Rig::new();
    rig.attach(
        OTHER,
        MemoryTarget::new().with_access(crate::access::Access::ReadOnly),
    );
    let gate = rig.epoch(1);
    let epoch = rig.epoch(2);
    let refused = rig
        .write_with(OTHER, epoch, 0, &[7; 4], gated(gate))
        .expect_err("a write to a read-only target");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::AccessDenied)
    ));
    assert_eq!(refused.buffer, Some(vec![7; 4]), "handed back");
    let refused = rig
        .write_with(OTHER, epoch, 0, &[7; 4], WriteOptions::new())
        .expect_err("ungated, the push refuses it");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::AccessDenied)
    ));

    // Nothing was held and no id was spent: the next accepted operation takes the first.
    let first = rig
        .read_with(0, 1, ReadOptions::new().gate(gate))
        .expect("a read of the read-write target is held");
    assert_eq!(first.seq, 0);
    assert_eq!(rig.core.close().held.len(), 1);
}

#[test]
fn the_cores_own_checks_come_before_the_backends() {
    let mut rig = Rig::new();
    let gate = rig.epoch(1);
    let unknown = crate::types::IoTargetKey(99);
    let refused = rig
        .core
        .write(
            unknown,
            0,
            SubmissionData::Registered(REGION),
            rig.epoch(2),
            "unknown",
            gated(gate),
            &mut rig.backend,
        )
        .expect_err("an unknown target");
    assert!(
        matches!(refused.reason, PushRefusal::UnknownFile(key) if key == unknown),
        "the core's refusal, not the backend's UnknownTarget: {:?}",
        refused.reason
    );

    rig.seal(3);
    rig.run();
    let refused = rig
        .core
        .write(
            KEY,
            0,
            SubmissionData::Registered(REGION),
            rig.epoch(2),
            "sealed",
            gated(gate),
            &mut rig.backend,
        )
        .expect_err("a sealed epoch");
    assert!(
        matches!(refused.reason, PushRefusal::Sealed { .. }),
        "the seal is checked before the backend: {:?}",
        refused.reason
    );
}

#[test]
fn a_released_write_the_backend_refuses_for_good_completes_as_failed_and_is_nullified() {
    let mut rig = Rig::new();
    rig.attach(OTHER, MemoryTarget::new());
    let lineage = rig.core.mint_lineage(None);
    let gate = Epoch::new(lineage, 1);
    rig.write_with(OTHER, gate, 0, b"g", WriteOptions::new())
        .expect("accepted");
    let held = rig
        .write_with(KEY, rig.epoch(1), 0, b"held", gated(gate))
        .expect("held");
    // The held write's target leaves the backend, so its release is refused for good.
    rig.backend.detach(KEY).expect("attached");
    rig.seal_in(lineage, 1);
    rig.run();
    assert!(rig.core.retry().is_empty(), "ended, not kept");
    assert_eq!(rig.backend.queued(), 0, "and never pushed again");
    let entries = rig.entries();
    let released = entries
        .iter()
        .find_map(|entry| match entry {
            Entry::Op(op) if op.id == held => Some(op),
            _ => None,
        })
        .expect("the released write completes");
    let Outcome::Failed(fault) = &released.outcome else {
        panic!("failed: {:?}", released.outcome);
    };
    assert_eq!(*refusal(fault), RefusalCause::UnknownTarget);
    assert!(
        matches!(fault.error, rc_errors::ErrorReport::Static(_)),
        "a cause carrying nothing is reported by a static"
    );
    assert_eq!(fault.code, None, "the cause carries no code of its own");
    assert_eq!(released.buffer.as_deref(), Some(b"held".as_slice()));
    assert_eq!(released.context, "write");

    // Nullified, as a write its target failed is: it holds nothing of its epoch back.
    rig.backend
        .attach(KEY, MemoryTarget::new())
        .expect("a new key");
    rig.seal(1);
    rig.run();
    assert_eq!(
        shown(&mut rig).last().map(String::as_str),
        Some("durable 1")
    );
}

#[test]
fn a_released_read_the_backend_refuses_for_good_completes_as_failed() {
    let mut rig = Rig::new();
    rig.attach(OTHER, MemoryTarget::new());
    rig.write_to(OTHER, 1, 0, b"g").expect("accepted");
    let held = rig
        .read_with(0, 4, ReadOptions::new().gate(rig.epoch(1)))
        .expect("held");
    rig.backend.detach(KEY).expect("attached");
    rig.seal(1);
    rig.run();
    let entries = rig.entries();
    let Some(Entry::Op(read)) = entries.last() else {
        panic!("the read completes last: {entries:?}");
    };
    assert_eq!((read.id, read.kind), (held, OpKind::Read));
    let Outcome::Failed(fault) = &read.outcome else {
        panic!("failed: {:?}", read.outcome);
    };
    assert_eq!(*refusal(fault), RefusalCause::UnknownTarget);
    assert_eq!(read.buffer.as_deref(), Some([0; 4].as_slice()));
    assert!(rig.core.retry().is_empty());
}

#[test]
fn a_commit_the_backend_refuses_for_good_fails_its_seals_flush() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.seal(1);
    rig.backend.submit().expect("submit");
    let write = rig.backend.pop().expect("never fails").expect("the write");
    // Gone before the commit the write's completion makes due is pushed.
    rig.backend.detach(KEY).expect("attached");
    let actions = rig.core.complete(write);
    assert_eq!(actions.submit.len(), 1, "the commit is due");
    rig.act(actions);
    assert!(rig.core.retry().is_empty(), "ended, not kept");
    let entries = rig.entries();
    let Some(Entry::Failed(failed)) = entries.last() else {
        panic!("a failure: {entries:?}");
    };
    let FailureCause::Commit { target, fault } = &failed.cause else {
        panic!("a commit's failure: {:?}", failed.cause);
    };
    assert_eq!(*target, KEY);
    assert_eq!(*refusal(fault), RefusalCause::UnknownTarget);
    assert_eq!(failed.suspect.writes().len(), 1);
    assert!(matches!(
        rig.core.epoch_state(rig.epoch(1)),
        Ok(EpochState::Blocked(_))
    ));
}

#[test]
fn a_retryable_refusal_is_kept_and_any_other_ends_the_submission() {
    // Both directions of `is_retryable`, through `refused`: a commit refused for each cause.
    let causes = [
        (RefusalCause::SubmitNeeded, true),
        (RefusalCause::DrainNeeded, true),
        (RefusalCause::SubmitAndDrainNeeded, true),
        (RefusalCause::Busy, true),
        (RefusalCause::UnknownTarget, false),
        (RefusalCause::Unsupported, false),
        (RefusalCause::Invalid, false),
    ];
    for (cause, kept) in causes {
        let mut rig = Rig::new();
        rig.write(1, 0, b"x").expect("accepted");
        rig.seal(1);
        rig.backend.submit().expect("submit");
        let write = rig.backend.pop().expect("never fails").expect("the write");
        let actions = rig.core.complete(write);
        let commit = actions.submit.into_iter().next().expect("the commit");
        let due = rig.core.refused(crate::backend::Refused {
            cause: cause.clone(),
            submission: commit,
        });
        assert!(due.submit.is_empty(), "{cause}: nothing to push at once");
        assert_eq!(rig.core.retry().submit.len(), usize::from(kept), "{cause}");
        let failed = rig
            .entries()
            .iter()
            .any(|entry| matches!(entry, Entry::Failed(_)));
        assert_eq!(
            failed, !kept,
            "{cause}: ended means the seal's flush failed"
        );
    }
}
