// Copyright (c) 2026 Mike Grier

use std::collections::HashSet;

use super::{Key, ReadKey, TargetKey, WriteKey};
use crate::access::{Access, HasData, ReadOnly, ReadWrite, WriteOnly};
use crate::types::{AddTargetCause, IoTargetKey};

const ALL: [Access; 3] = [Access::ReadOnly, Access::WriteOnly, Access::ReadWrite];

fn pushes_a_write<K: WriteKey>(key: K) -> IoTargetKey {
    key.untyped()
}

fn pushes_a_read<K: ReadKey>(key: K) -> IoTargetKey {
    key.untyped()
}

#[test]
fn an_untyped_key_declares_nothing_and_is_the_number_it_holds() {
    for number in [0, 1, 7, u64::MAX] {
        let key = IoTargetKey(number);
        assert_eq!(TargetKey::untyped(&key), key);
        assert_eq!(key.declared(), None);
    }
}

#[test]
fn an_untyped_key_may_write_and_read_so_the_core_judges_it_when_it_is_pushed() {
    let key = IoTargetKey(3);
    assert_eq!(pushes_a_write(key), key);
    assert_eq!(pushes_a_read(key), key);
}

#[test]
fn a_typed_key_declares_the_access_its_marker_names() {
    let number = IoTargetKey(9);
    assert_eq!(
        Key::<ReadOnly>::new(number).declared(),
        Some(Access::ReadOnly)
    );
    assert_eq!(
        Key::<WriteOnly>::new(number).declared(),
        Some(Access::WriteOnly)
    );
    assert_eq!(
        Key::<ReadWrite>::new(number).declared(),
        Some(Access::ReadWrite)
    );
}

#[test]
fn a_typed_key_keeps_the_number_through_every_way_of_taking_it_out() {
    let number = IoTargetKey(41);
    let key = Key::<ReadWrite>::new(number);
    assert_eq!(key.untyped(), number);
    assert_eq!(TargetKey::untyped(&key), number);
    assert_eq!(IoTargetKey::from(key), number);
}

#[test]
fn a_key_that_can_write_is_a_write_key_and_one_that_can_read_is_a_read_key() {
    let number = IoTargetKey(5);
    assert_eq!(pushes_a_write(Key::<WriteOnly>::new(number)), number);
    assert_eq!(pushes_a_write(Key::<ReadWrite>::new(number)), number);
    assert_eq!(pushes_a_read(Key::<ReadOnly>::new(number)), number);
    assert_eq!(pushes_a_read(Key::<ReadWrite>::new(number)), number);
}

#[test]
fn a_key_that_cannot_write_or_cannot_read_is_refused_to_the_compiler() {
    // `Key<ReadOnly>` as a `WriteKey` and `Key<WriteOnly>` as a `ReadKey` do not compile; the
    // doctests on the traits hold that. What can be held here is that each is still a registrable
    // key, so the refusal is of the push and not of the key's existence.
    fn registrable<K: TargetKey>(_: K) {}
    registrable(Key::<ReadOnly>::new(IoTargetKey(1)));
    registrable(Key::<WriteOnly>::new(IoTargetKey(1)));
}

#[test]
fn typed_keys_compare_and_hash_by_their_number_alone() {
    let a = Key::<ReadWrite>::new(IoTargetKey(1));
    let same = Key::<ReadWrite>::new(IoTargetKey(1));
    let other = Key::<ReadWrite>::new(IoTargetKey(2));
    assert_eq!(a, same);
    assert_ne!(a, other);
    assert_eq!(a, a.clone());
    let set: HashSet<Key<ReadWrite>> = [a, same, other].into_iter().collect();
    assert_eq!(set.len(), 2);
}

#[test]
fn a_typed_keys_debug_shows_its_number_and_what_it_declares() {
    let text = format!("{:?}", Key::<ReadOnly>::new(IoTargetKey(12)));
    assert!(text.contains("12"), "{text}");
    assert!(text.contains("ReadOnly"), "{text}");
}

#[test]
fn a_target_is_admitted_under_a_key_whose_declaration_it_covers() {
    // Every pair of declared and stated access, in one table: the target must be able to do all
    // that the key declares, and a key that declares less than it can do is a narrower view.
    let expected = [
        // (declared, actual, admitted)
        (Access::ReadOnly, Access::ReadOnly, true),
        (Access::ReadOnly, Access::WriteOnly, false),
        (Access::ReadOnly, Access::ReadWrite, true),
        (Access::WriteOnly, Access::ReadOnly, false),
        (Access::WriteOnly, Access::WriteOnly, true),
        (Access::WriteOnly, Access::ReadWrite, true),
        (Access::ReadWrite, Access::ReadOnly, false),
        (Access::ReadWrite, Access::WriteOnly, false),
        (Access::ReadWrite, Access::ReadWrite, true),
    ];
    for (declared, actual, admitted) in expected {
        let key = match declared {
            Access::ReadOnly => Key::<ReadOnly>::new(IoTargetKey(1)).admitted_by(actual),
            Access::WriteOnly => Key::<WriteOnly>::new(IoTargetKey(1)).admitted_by(actual),
            Access::ReadWrite => Key::<ReadWrite>::new(IoTargetKey(1)).admitted_by(actual),
        };
        let wanted = if admitted {
            Ok(())
        } else {
            Err(AddTargetCause::AccessNotCovered { declared, actual })
        };
        assert_eq!(key, wanted, "declared {declared}, stated {actual}");
    }
}

#[test]
fn an_untyped_key_is_admitted_by_every_target() {
    for actual in ALL {
        assert_eq!(IoTargetKey(1).admitted_by(actual), Ok(()), "{actual}");
    }
}

#[test]
fn a_refusal_names_what_was_declared_and_what_the_target_states() {
    let cause = Key::<ReadWrite>::new(IoTargetKey(1))
        .admitted_by(Access::ReadOnly)
        .expect_err("a read-only target does not cover read and write");
    let text = cause.to_string();
    assert!(text.contains("read and write"), "{text}");
    assert!(text.contains("read only"), "{text}");
}

#[test]
fn the_marker_a_key_is_made_with_decides_what_it_declares_and_nothing_else_does() {
    fn declared<A: HasData>() -> Option<Access> {
        Key::<A>::new(IoTargetKey(0)).declared()
    }
    assert_eq!(declared::<ReadOnly>(), Some(ReadOnly::ACCESS));
    assert_eq!(declared::<WriteOnly>(), Some(WriteOnly::ACCESS));
    assert_eq!(declared::<ReadWrite>(), Some(ReadWrite::ACCESS));
}
