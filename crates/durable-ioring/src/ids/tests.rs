// Copyright (c) 2026 Mike Grier

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::contract::Identities;
use crate::ids::{DioringIds, FailureId, FailureToken, InstanceId, Lineage, OpId};

#[test]
fn every_instance_id_is_new() {
    let ids: Vec<InstanceId> = (0..100).map(|_| InstanceId::next()).collect();
    let distinct: HashSet<_> = ids.iter().copied().collect();
    assert_eq!(distinct.len(), ids.len());
}

#[test]
fn operations_order_by_push_order_within_an_instance() {
    let instance = InstanceId::next();
    let first = OpId { instance, seq: 1 };
    let second = OpId { instance, seq: 2 };
    assert_eq!(first.partial_cmp(&second), Some(Ordering::Less));
    assert!(first < second);
    assert_eq!(first.partial_cmp(&first), Some(Ordering::Equal));
}

#[test]
fn operations_of_two_instances_are_incomparable() {
    let a = OpId {
        instance: InstanceId::next(),
        seq: 1,
    };
    let b = OpId {
        instance: InstanceId::next(),
        seq: 2,
    };
    assert_eq!(a.partial_cmp(&b), None);
    assert_ne!(a, b);
}

#[test]
fn identities_carry_their_instance() {
    let (one, two) = (InstanceId::next(), InstanceId::next());
    assert_ne!(
        Lineage {
            instance: one,
            seq: 0
        },
        Lineage {
            instance: two,
            seq: 0
        }
    );
    assert_ne!(
        FailureId {
            instance: one,
            seq: 0
        },
        FailureId {
            instance: two,
            seq: 0
        }
    );
}

#[test]
fn a_token_names_its_failure() {
    let id = FailureId {
        instance: InstanceId::next(),
        seq: 7,
    };
    let token = FailureToken { id };
    assert_eq!(token.id(), id);
    assert_eq!(<DioringIds<u64> as Identities>::token_id(&token), id);
}

/// An epoch-id type with no `Hash` and no `Default`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Bare(u8);

#[test]
fn the_marker_is_copy_eq_and_hash_whatever_the_epoch_id() {
    let a = DioringIds::<Bare>(std::marker::PhantomData);
    let b = a;
    assert_eq!(a, b);
    let set: HashSet<_> = [a, b].into_iter().collect();
    assert_eq!(set.len(), 1);
    assert_eq!(format!("{a:?}"), "DioringIds");
    assert_eq!(size_of::<DioringIds<Bare>>(), 0);
}
