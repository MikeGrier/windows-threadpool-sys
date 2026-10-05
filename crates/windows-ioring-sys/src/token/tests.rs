// Copyright (c) Mike Grier.

//! Tests for [`OperationId`], the identity that survived `Token`'s retirement
//! (`D-71`, `M28.4.1d.3`).
//!
//! `Token`'s own tests went with it, and most had nothing left to say: they
//! were about claiming, leaking, and the forget-on-drop mechanism, none of
//! which exists once the ring holds what an operation carries. Two of them
//! were about *identity* rather than ownership, and those are kept here in
//! their new form -- distinctness within a ring, and the cross-ring collision
//! that makes `ring_id` necessary.

use super::OperationId;
use crate::accounting::Accounting;

/// Every operation on one ring gets its own `UserData`.
///
/// The property a cancel target depends on: naming an operation by its id has
/// to name exactly one.
#[test]
fn each_operation_on_a_ring_gets_a_distinct_id() {
    let mut accounting = Accounting::new();
    let ring_id = accounting.ring_id();
    let ids: Vec<OperationId> = (0..8)
        .map(|_| {
            OperationId::new(
                accounting.reserve_user_data().expect("the ledger has room"),
                ring_id,
            )
        })
        .collect();

    let mut seen: Vec<usize> = ids.iter().map(OperationId::user_data).collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), before, "two operations shared a UserData");
}

/// The same integer legitimately occurs on two rings, and `ring_id` is what
/// tells them apart.
///
/// This is why the identity carries a ring at all. Every ring counts from the
/// same place, so a caller correlating across rings sees a collision that is
/// not one -- and `IoRing::stow` debug-asserts against exactly this, which
/// only means something if the collision is real.
#[test]
fn the_same_user_data_on_two_rings_is_two_identities() {
    let mut first = Accounting::new();
    let mut second = Accounting::new();
    let a = OperationId::new(first.reserve_user_data().expect("room"), first.ring_id());
    let b = OperationId::new(second.reserve_user_data().expect("room"), second.ring_id());

    assert_eq!(
        a.user_data(),
        b.user_data(),
        "two fresh rings must hand out the same first UserData, or this test proves nothing"
    );
    assert_ne!(a, b, "the same UserData on two rings is two identities");
    assert_ne!(a.ring_id(), b.ring_id());
}

/// The `Display` form is the hexadecimal `UserData`, which is how every error
/// message in this crate names an operation.
#[test]
fn display_is_the_hexadecimal_user_data() {
    let accounting = Accounting::new();
    let id = OperationId::new(0x2A, accounting.ring_id());
    assert_eq!(id.to_string(), "0x2a");
}

/// `OperationId` is `Copy`, which is what makes it a name rather than a
/// capability: handing it out cannot take it away from anyone.
#[test]
fn an_operation_id_is_copy_and_compares_by_value() {
    let accounting = Accounting::new();
    let id = OperationId::new(7, accounting.ring_id());
    let copied = id;
    assert_eq!(id, copied);
    assert_eq!(id.user_data(), copied.user_data());
}
