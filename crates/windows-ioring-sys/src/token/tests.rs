// Copyright (c) 2026 Mike Grier
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Token;
use crate::IoRing;
use crate::buf::IoBuf;

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

/// None of these tests ever actually submits anything to `ring` (M3 has not
/// been built yet), so a token minted here never gets a real completion.
/// `IoRing::run_down` -- which its `Drop` calls -- waits for exactly that, so
/// every test must call `record_completion` once per token it minted before
/// letting `ring` drop, or teardown would hang waiting for a completion that
/// will never arrive.
fn settle(ring: &mut IoRing) {
    while ring.outstanding() > 0 {
        ring.record_completion();
    }
}

#[test]
fn dropping_an_unclaimed_token_never_runs_the_buffers_destructor() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    let (buffer, dropped) = tracked_buffer();
    let token = Token::new(&mut ring, buffer).expect("mint token");

    drop(token);

    assert!(
        !dropped.load(Ordering::SeqCst),
        "an unclaimed token's Drop must forget the buffer, not free it -- \
         a real IoRing may still be writing through it"
    );

    settle(&mut ring);
    drop(ring);
    // The leak is real and permanent: nothing later runs the destructor
    // either, including the ring's own teardown.
    assert!(!dropped.load(Ordering::SeqCst));
}

#[test]
fn claiming_a_token_returns_the_buffer_for_normal_disposal() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    let (buffer, dropped) = tracked_buffer();
    let token = Token::new(&mut ring, buffer).expect("mint token");
    let id = token.id();

    let claimed = token.claim_if(id).expect("id matches itself");
    assert!(
        !dropped.load(Ordering::SeqCst),
        "claiming must not itself drop the buffer"
    );

    drop(claimed);
    assert!(
        dropped.load(Ordering::SeqCst),
        "the caller's own drop of the returned buffer must run normally"
    );

    settle(&mut ring);
}

#[test]
fn claim_if_rejects_a_mismatched_user_data_and_returns_the_token_unchanged() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    let (buffer, dropped) = tracked_buffer();
    let token = Token::new(&mut ring, buffer).expect("mint token");
    let real_id = token.id();

    let token = token
        .claim_if(real_id.wrapping_add(1))
        .expect_err("a stale id must not claim this token");
    assert_eq!(
        token.id(),
        real_id,
        "the rejected token is handed back unchanged"
    );
    assert!(!dropped.load(Ordering::SeqCst));

    // It can still be claimed correctly afterwards.
    let claimed = token.claim_if(real_id).expect("the real id still works");
    drop(claimed);
    assert!(dropped.load(Ordering::SeqCst));

    settle(&mut ring);
}

#[test]
fn each_token_on_a_ring_gets_a_distinct_id() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    let (a, _) = tracked_buffer();
    let (b, _) = tracked_buffer();
    let token_a = Token::new(&mut ring, a).expect("mint token a");
    let token_b = Token::new(&mut ring, b).expect("mint token b");
    assert_ne!(token_a.id(), token_b.id());

    drop(token_a);
    drop(token_b);
    settle(&mut ring);
}

#[test]
fn minting_a_token_increments_the_rings_outstanding_count() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    assert_eq!(ring.outstanding(), 0);
    let (buffer, _dropped) = tracked_buffer();
    let token = Token::new(&mut ring, buffer).expect("mint token");
    assert_eq!(ring.outstanding(), 1);

    // Dropping the token does not, by itself, tell the ring the operation is
    // done -- only observing a real completion does (M2.4); this token was
    // never actually submitted to anything, so nothing ever will.
    drop(token);
    assert_eq!(
        ring.outstanding(),
        1,
        "outstanding tracks completions observed, not tokens dropped"
    );

    ring.record_completion();
    assert_eq!(ring.outstanding(), 0);
}
