// Copyright (c) 2026 Mike Grier
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Token;
use crate::accounting::Accounting;
use crate::buf::IoBuf;
use crate::ring::Completion;

/// A buffer that records whether its destructor ran, to distinguish "leaked
/// (forgotten)" from "dropped (freed)" -- the exact distinction M2.3 exists to
/// get right.
#[derive(Debug)]
struct DropTracking {
    data: Vec<u8>,
    dropped: Arc<AtomicBool>,
}

impl Drop for DropTracking {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

// SAFETY: the bytes live in `data`'s heap allocation, independent of where
// this wrapper struct sits; the length is fixed once constructed.
unsafe impl IoBuf for DropTracking {
    fn stable_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.data.len()
    }
}

fn tracked_buffer() -> (DropTracking, Arc<AtomicBool>) {
    let dropped = Arc::new(AtomicBool::new(false));
    (
        DropTracking {
            data: vec![0_u8; 8],
            dropped: dropped.clone(),
        },
        dropped,
    )
}

/// Until M24.7 these tests each opened a real `IoRing`, and the ring was
/// purely a liability. None of them ever submitted anything, so a token minted
/// here never got a real completion -- and `IoRing::run_down`, which `Drop`
/// calls, waits for exactly that. Every test therefore had to call
/// `record_completion` once per token through a `settle` helper, or teardown
/// would hang waiting for a completion that was never coming.
///
/// `Token::new` now takes the ring's ledger rather than the ring, because an
/// identity and a ring id are all it ever needed. The helper is gone with the
/// hazard it existed to work around, and these tests open nothing.
#[test]
fn dropping_an_unclaimed_token_never_runs_the_buffers_destructor() {
    let mut ledger = Accounting::new();
    let (buffer, dropped) = tracked_buffer();
    let token = Token::new(&mut ledger, buffer).expect("mint token");

    drop(token);

    assert!(
        !dropped.load(Ordering::SeqCst),
        "an unclaimed token's Drop must forget the buffer, not free it -- \
         a real IoRing may still be writing through it"
    );

    // The leak is real and permanent: nothing later runs the destructor
    // either, including the ring's own teardown.
    assert!(!dropped.load(Ordering::SeqCst));
}

#[test]
fn claiming_a_token_returns_the_buffer_for_normal_disposal() {
    let mut ledger = Accounting::new();
    let (buffer, dropped) = tracked_buffer();
    let token = Token::new(&mut ledger, buffer).expect("mint token");
    let id = token.id();

    let completion = Completion::synthetic(id, 0, ledger.ring_id());
    let claimed = token.claim_if(&completion).expect("id matches itself");
    assert!(
        !dropped.load(Ordering::SeqCst),
        "claiming must not itself drop the buffer"
    );

    drop(claimed);
    assert!(
        dropped.load(Ordering::SeqCst),
        "the caller's own drop of the returned buffer must run normally"
    );
}

#[test]
fn claim_if_rejects_a_mismatched_user_data_and_returns_the_token_unchanged() {
    let mut ledger = Accounting::new();
    let (buffer, dropped) = tracked_buffer();
    let token = Token::new(&mut ledger, buffer).expect("mint token");
    let real_id = token.id();

    let mismatched = Completion::synthetic(real_id.wrapping_add(1), 0, ledger.ring_id());
    let token = token
        .claim_if(&mismatched)
        .expect_err("a stale id must not claim this token");
    assert_eq!(
        token.id(),
        real_id,
        "the rejected token is handed back unchanged"
    );
    assert!(!dropped.load(Ordering::SeqCst));

    // It can still be claimed correctly afterwards.
    let matching = Completion::synthetic(real_id, 0, ledger.ring_id());
    let claimed = token.claim_if(&matching).expect("the real id still works");
    drop(claimed);
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn each_token_on_a_ring_gets_a_distinct_id() {
    let mut ledger = Accounting::new();
    let (a, _) = tracked_buffer();
    let (b, _) = tracked_buffer();
    let token_a = Token::new(&mut ledger, a).expect("mint token a");
    let token_b = Token::new(&mut ledger, b).expect("mint token b");
    assert_ne!(token_a.id(), token_b.id());

    drop(token_a);
    drop(token_b);
}

#[test]
fn minting_a_token_increments_the_ledgers_outstanding_count() {
    let mut ledger = Accounting::new();
    assert_eq!(ledger.outstanding(), 0);
    let (buffer, _dropped) = tracked_buffer();
    let token = Token::new(&mut ledger, buffer).expect("mint token");
    assert_eq!(ledger.outstanding(), 1);

    // Dropping the token does not, by itself, tell the ledger the operation is
    // done -- only observing a real completion does (M2.4); this token was
    // never actually submitted to anything, so nothing ever will.
    drop(token);
    assert_eq!(
        ledger.outstanding(),
        1,
        "outstanding tracks completions observed, not tokens dropped"
    );

    ledger.record_completion();
    assert_eq!(ledger.outstanding(), 0);
}

#[test]
fn claim_if_rejects_a_matching_user_data_from_a_different_ring() {
    let mut ledger_a = Accounting::new();
    let ledger_b = Accounting::new();
    let (buffer, dropped) = tracked_buffer();
    let token = Token::new(&mut ledger_a, buffer).expect("mint token on ring a");
    let id = token.id();

    // Both rings mint `UserData` from their own counter starting at zero, so
    // this is a real coincidence a naive `id`-only check would miss (PR #20
    // review response): the completion carries the *same* `UserData` value
    // as `token`, but from `ring_b`, not `ring_a`.
    let wrong_ring = Completion::synthetic(id, 0, ledger_b.ring_id());
    let token = token
        .claim_if(&wrong_ring)
        .expect_err("a completion from a different ring must not claim this token");
    assert!(!dropped.load(Ordering::SeqCst));

    let right_ring = Completion::synthetic(id, 0, ledger_a.ring_id());
    let claimed = token
        .claim_if(&right_ring)
        .expect("the same ring's completion still claims it");
    drop(claimed);
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn a_tokens_debug_names_its_operation() {
    // `Token`'s `Debug` is hand-written rather than derived, because deriving
    // would demand `T: Debug` from a caller's buffer type, and the id is the
    // only part worth printing (D-4). M18.3 showed nothing asserted either
    // half of that choice: the impl could return an empty string unnoticed.
    let mut ledger = Accounting::new();

    // A buffer type that is deliberately *not* `Debug`, which is the constraint
    // that forced the hand-written impl in the first place.
    struct NotDebug(Vec<u8>);
    // SAFETY: the bytes live in a heap allocation whose address is independent
    // of where this value sits, and nothing here reallocates.
    unsafe impl IoBuf for NotDebug {
        fn stable_ptr(&self) -> *const u8 {
            self.0.as_ptr()
        }
        fn bytes_len(&self) -> usize {
            self.0.len()
        }
    }

    let token = Token::new(&mut ledger, NotDebug(vec![0_u8; 8])).expect("mint a token");
    let id = token.id();
    let rendered = format!("{token:?}");

    assert!(
        rendered.contains(&id.to_string()),
        "a token's Debug must name the operation it will claim, but was {rendered:?}"
    );
    assert!(
        rendered.contains("Token"),
        "a token's Debug must name its type, but was {rendered:?}"
    );

    drop(token);
}
