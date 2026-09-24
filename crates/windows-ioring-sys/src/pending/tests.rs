// Copyright (c) 2026 Mike Grier
//! Tests for the `M23.3` spike.
//!
//! # What these establish, and what they leave to a real ring
//!
//! Everything about the *rule* -- that an unclaimed token is loud, that saying
//! so deliberately is quiet, that a checked map reports what a raw `HashMap`
//! cannot -- needs only `push`, so it is settled here, hermetically, with a
//! ledger and no ring at all.
//!
//! `claim` is the one operation that needs a real `Completion`, and a
//! `Completion` carries a `RingId` that only a ring mints. Rather than add a
//! synthetic constructor whose fidelity would then itself need arguing, that
//! half is validated by converting a real consumer -- stronger evidence anyway,
//! since it answers whether the type *fits* as well as whether it works.

use crate::Token;
use crate::accounting::Accounting;
use crate::contract::Violation;

use super::Pending;

/// A payload that is `Send + 'static` and carries nothing interesting.
#[derive(Debug, PartialEq, Eq)]
struct Payload(u32);

fn token(ledger: &mut Accounting, value: u32) -> Token<Payload> {
    Token::new(ledger, Payload(value)).expect("mint a token")
}

/// The map counts what it holds.
#[test]
fn pushing_makes_an_operation_outstanding() {
    let mut ledger = Accounting::new();
    let mut pending: Pending<Payload> = Pending::new();
    assert!(pending.is_empty());

    let first = token(&mut ledger, 1);
    let first_id = first.id();
    pending.push(first, ());
    let second = token(&mut ledger, 2);
    let second_id = second.id();
    pending.push(second, ());
    assert_eq!(pending.len(), 2);

    // Deliberately emptied, or `Drop` would fire -- which is the next test.
    assert!(pending.abandon(first_id));
    assert!(pending.abandon(second_id));
}

/// **The claim this spike exists to test.** An unclaimed token is loud.
///
/// A token dropped unclaimed leaks its buffer on purpose -- `Token` forgets the
/// value rather than freeing it, because the kernel may still be writing. That
/// is invisible at runtime: the program keeps working and loses memory. A raw
/// `HashMap` cannot notice; this can.
#[test]
#[should_panic(expected = "dropped unclaimed")]
fn dropping_a_map_that_still_holds_tokens_panics() {
    let mut ledger = Accounting::new();
    let mut pending: Pending<Payload> = Pending::new();
    pending.push(token(&mut ledger, 1), ());
    // Dropped here, still holding one token.
}

/// Saying so deliberately is quiet.
///
/// The other direction of the same guard, and the one that makes it usable: a
/// consumer tearing down early has genuinely abandoned these tokens and must be
/// able to say so without tripping an alarm meant for forgetting.
#[test]
fn abandoning_every_token_leaves_the_drop_silent() {
    let mut ledger = Accounting::new();
    let mut pending: Pending<Payload> = Pending::new();
    let held = token(&mut ledger, 1);
    let held_id = held.id();
    pending.push(held, ());
    assert!(pending.abandon(held_id));
    // Drops silently.
}

/// `finish` reports what was left, and leaves `Drop` nothing to say.
#[test]
fn finish_reports_unclaimed_tokens_instead_of_panicking() {
    let mut ledger = Accounting::new();
    let mut pending: Pending<Payload> = Pending::new();
    let held = token(&mut ledger, 7);
    let held_id = held.id();
    pending.push(held, ());

    let violations = pending.finish();
    assert_eq!(
        violations,
        vec![Violation::LeakedToken { user_data: held_id }]
    );
}

/// A map that claimed everything finishes clean.
#[test]
fn an_empty_map_finishes_with_no_violations() {
    let pending: Pending<Payload> = Pending::new();
    assert!(pending.finish().is_empty());
}

/// A checked map's oracle sees the pushes the map recorded.
///
/// This is the wiring the spike is really about: one call site updated both,
/// where every real consumer today updates them separately and can drift.
#[test]
fn a_checked_map_drives_its_own_contract() {
    let mut ledger = Accounting::new();
    let mut pending: Pending<Payload> = Pending::checked();
    let held = token(&mut ledger, 3);
    let held_id = held.id();
    pending.push(held, ());

    let contract = pending.contract().expect("a checked map has one");
    assert!(
        contract
            .check_quiescent()
            .contains(&Violation::Outstanding { user_data: held_id }),
        "the oracle should know about a push nobody told it about by hand"
    );

    let violations = pending.finish();
    assert!(violations.contains(&Violation::Outstanding { user_data: held_id }));
}

/// An unchecked map has no oracle, and says so.
#[test]
fn an_unchecked_map_has_no_contract() {
    let pending: Pending<Payload> = Pending::new();
    assert!(pending.contract().is_none());
    let _ = pending.finish();
}

/// The sidecar comes back with the payload.
///
/// The generic the census forced: two thirds of the real sites carry
/// per-operation data beside the token, so a map holding only tokens would
/// serve the minority.
#[test]
fn a_sidecar_is_stored_alongside_the_token() {
    let mut ledger = Accounting::new();
    let mut pending: Pending<Payload, (u32, &'static str)> = Pending::new();
    let held = token(&mut ledger, 1);
    let held_id = held.id();
    pending.push(held, (17, "slot"));
    assert_eq!(pending.len(), 1);
    assert!(pending.abandon(held_id));
}

/// Two tokens cannot share an identity.
#[test]
#[should_panic(expected = "already pending")]
fn pushing_a_duplicate_identity_panics() {
    let mut ledger = Accounting::new();
    let mut pending: Pending<Payload> = Pending::new();
    let held = token(&mut ledger, 1);
    let id = held.id();
    pending.push(held, ());

    // A second ledger mints from zero again, so this collides by construction.
    let mut other_ledger = Accounting::new();
    let clash = token(&mut other_ledger, 2);
    assert_eq!(clash.id(), id, "a fresh ledger restarts identities");
    pending.push(clash, ());
}

/// `Debug` shows identities and counts, never payloads.
#[test]
fn debug_does_not_print_payloads() {
    let mut ledger = Accounting::new();
    let mut pending: Pending<Payload> = Pending::new();
    let held = token(&mut ledger, 0xBEEF);
    let held_id = held.id();
    pending.push(held, ());

    let rendered = format!("{pending:?}");
    assert!(rendered.contains("outstanding"), "{rendered}");
    assert!(!rendered.contains("BEEF"), "payload leaked into Debug");
    assert!(!rendered.contains("beef"), "payload leaked into Debug");

    assert!(pending.abandon(held_id));
}
