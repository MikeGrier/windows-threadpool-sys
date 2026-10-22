// Copyright (c) 2026 Mike Grier
//! Typed keys against a real ring (DI-13.10, DI-D-66): registration compares what a key declares
//! with what the target states, and a typed key then carries writes and reads like any other.
//!
//! That a write through a `Key<ReadOnly>` does not compile is held by the doctests on `WriteKey`
//! and `ReadKey` in `durable-ioring`; what is held here is what only a ring can show.

use durable_ioring::{
    Access, AddTargetCause, DurableRing, IoTarget, IoTargetKey, Key, OpKind, PushRefusal, ReadOnly,
    ReadWrite, WriteOnly,
};

use super::pushes::{ADDED, Harness, instance, read_only, temp, transferred};
use super::{Ring, SetupRefusal, TargetSetup, refused, serves, setup};
use crate::windows_backend::Win32FileTarget;

/// A ring built from nothing, wrapped for pushes.
fn empty_harness() -> Harness {
    Harness::new(instance(Vec::new(), Vec::new()))
}

#[test]
fn typed_keys_register_a_target_and_carry_its_writes_and_reads() {
    let temp = temp(&[0; 32]);
    let mut harness = empty_harness();
    harness
        .ring
        .add_target(
            Key::<ReadWrite>::new(ADDED),
            Win32FileTarget::new(temp.open()),
        )
        .expect("a read-write target under a read-write key");

    let bytes = b"typed";
    let epoch = harness.epoch(1);
    let write = harness
        .ring
        .write(
            Key::<WriteOnly>::new(ADDED),
            4,
            bytes.to_vec(),
            harness.default.at(1),
            1,
        )
        .expect("a write through a key that can write");
    harness.pushed(write, OpKind::Write { epoch }, 5, 1);
    let written = harness.next();
    assert_eq!(transferred(&written), 5);

    let read = harness
        .ring
        .read(Key::<ReadOnly>::new(ADDED), 4, vec![0; 5], 2)
        .expect("a read through a key that can read");
    harness.pushed(read, OpKind::Read, 5, 2);
    let back = harness.next();
    assert_eq!(back.buffer.as_deref(), Some(&bytes[..]));
    harness.finish();
}

#[test]
fn a_key_that_declares_more_than_the_target_states_is_refused_and_the_target_comes_back() {
    let temp = temp(&[0; 8]);
    let mut harness = empty_harness();
    let target = Win32FileTarget::new(read_only(&temp));
    let error = harness
        .ring
        .add_target(Key::<ReadWrite>::new(ADDED), target)
        .expect_err("a read-only target is not covered by a read-write key");
    assert_eq!(error.key, ADDED);
    assert_eq!(
        error.cause,
        AddTargetCause::AccessNotCovered {
            declared: Access::ReadWrite,
            actual: Access::ReadOnly,
        }
    );
    assert_eq!(error.target.access(), Access::ReadOnly);
    assert!(
        harness.ring.engine().core.target(ADDED).is_none(),
        "nothing was registered, so the key is free"
    );

    harness
        .ring
        .add_target(Key::<ReadOnly>::new(ADDED), error.target)
        .expect("the target registers under a key it covers");
    assert!(harness.ring.engine().core.target(ADDED).is_some());
}

#[test]
fn a_key_that_declares_less_is_a_view_and_does_not_restrict_the_target() {
    let temp = temp(&[0; 16]);
    let mut harness = empty_harness();
    harness
        .ring
        .add_target(
            Key::<ReadOnly>::new(ADDED),
            Win32FileTarget::new(temp.open()),
        )
        .expect("a read-write target under a read-only key");

    // The declaration is the holder's; the ring judges what is pushed by the target.
    harness.write(ADDED, 0, b"still", 1, 3);
    assert_eq!(transferred(&harness.next()), 5);
    harness.finish();
}

#[test]
fn an_untyped_key_declares_nothing_so_any_target_registers_under_it() {
    let temp = temp(&[0; 8]);
    let mut harness = empty_harness();
    harness
        .ring
        .add_target(ADDED, Win32FileTarget::new(read_only(&temp)))
        .expect("an untyped key is admitted by a read-only target");
}

#[test]
fn a_key_already_in_use_is_refused_as_such_before_its_declaration_is_judged() {
    let (a, b) = (temp(&[0; 8]), temp(&[0; 8]));
    let mut harness = empty_harness();
    harness
        .ring
        .add_target(ADDED, Win32FileTarget::new(a.open()))
        .expect("the first target");
    let error = harness
        .ring
        .add_target(
            Key::<ReadWrite>::new(ADDED),
            Win32FileTarget::new(read_only(&b)),
        )
        .expect_err("the key is taken");
    assert_eq!(error.cause, AddTargetCause::KeyInUse);
}

#[test]
fn a_write_through_a_target_that_cannot_write_is_still_refused_at_the_push_by_an_untyped_key() {
    let temp = temp(&[0; 8]);
    let mut harness = empty_harness();
    harness
        .ring
        .add_target(
            Key::<ReadOnly>::new(ADDED),
            Win32FileTarget::new(read_only(&temp)),
        )
        .expect("a read-only target under a read-only key");
    let refused = harness
        .ring
        .write(ADDED, 0, b"no".to_vec(), harness.default.at(1), 0)
        .expect_err("the untyped key is judged when it is pushed");
    assert!(matches!(refused.reason, PushRefusal::Backend(_)));
}

#[test]
fn construction_accepts_a_key_the_target_covers() {
    let temp = temp(&[0; 8]);
    let ring = Ring::new(setup(
        vec![TargetSetup::new(
            Key::<ReadOnly>::new(IoTargetKey(1)),
            Win32FileTarget::new(temp.open()),
        )],
        Vec::new(),
        None,
    ))
    .expect("a read-only key over a read-write target")
    .0;
    assert!(ring.engine().core.target(IoTargetKey(1)).is_some());
}

#[test]
fn construction_refuses_a_key_that_declares_more_than_its_target_states_and_all_comes_back() {
    let (a, b) = (temp(&[0; 8]), temp(&[0; 8]));
    let files = vec![
        TargetSetup::new(
            Key::<ReadWrite>::new(IoTargetKey(1)),
            Win32FileTarget::new(a.open()),
        ),
        TargetSetup::new(
            Key::<ReadWrite>::new(IoTargetKey(2)),
            Win32FileTarget::new(read_only(&b)),
        ),
    ];
    let error = refused(Ring::new(setup(files, vec![vec![1, 2]], serves(&["P"]))));
    match error.reason {
        SetupRefusal::AccessNotCovered {
            key,
            declared,
            actual,
        } => {
            assert_eq!(key, IoTargetKey(2));
            assert_eq!(declared, Access::ReadWrite);
            assert_eq!(actual, Access::ReadOnly);
        }
        other => panic!("refused for the wrong reason: {other:?}"),
    }
    assert_eq!(
        error
            .files
            .iter()
            .map(|file| (file.key, file.declared))
            .collect::<Vec<_>>(),
        [
            (IoTargetKey(1), Some(Access::ReadWrite)),
            (IoTargetKey(2), Some(Access::ReadWrite)),
        ],
        "the files come back in order, each with what its key declared"
    );
    assert_eq!(error.buffers, Some(vec![vec![1, 2]]));
    assert!(error.provider.is_some());
    drop(error.files);
    assert!(!a.is_held(), "no ring took a handle");
}

#[test]
fn a_construction_that_fails_later_hands_each_file_back_with_its_declaration() {
    let temp = temp(&[0; 8]);
    let mut given = setup(
        vec![
            TargetSetup::new(
                Key::<ReadOnly>::new(IoTargetKey(1)),
                Win32FileTarget::new(temp.open()),
            ),
            TargetSetup::new(IoTargetKey(2), Win32FileTarget::new(temp.open())),
        ],
        Vec::new(),
        None,
    );
    // A queue no system can allocate: the failure comes after the declarations were checked.
    given.submission_queue_size = u32::MAX;
    given.completion_queue_size = u32::MAX;
    let error = refused(Ring::new(given));
    assert!(
        matches!(error.reason, SetupRefusal::Ring(_)),
        "{:?}",
        error.reason
    );
    assert_eq!(
        error
            .files
            .iter()
            .map(|file| (file.key, file.declared))
            .collect::<Vec<_>>(),
        [
            (IoTargetKey(1), Some(Access::ReadOnly)),
            (IoTargetKey(2), None)
        ],
        "a typed key's declaration survives the hand-back and an untyped key stays untyped"
    );
}
