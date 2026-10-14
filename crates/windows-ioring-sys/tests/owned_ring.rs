// Copyright (c) 2026 Mike Grier
//! A ring sealed against operations it holds nothing for (M41.1, D-91).

#![cfg(windows)]

use std::time::Duration;

use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, IoRing, Owned, PushOptions, RegisteredSpan, SharedHandle,
};

mod common;

/// Generous: the bound is not what is under test.
const POP_BOUND: Duration = Duration::from_secs(10);
const LEN: usize = 64;
const SPAN: u32 = 64;

type Ring = IoRing<Vec<u8>, usize>;

/// A file of `LEN` bytes of `fill`, open for reading and writing, and the path that removes it.
fn fixture(tag: &str, fill: u8) -> (common::TempPath, SharedHandle) {
    let path = common::TempPath::new("owned-ring", tag);
    std::fs::write(&path, vec![fill; LEN]).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open");
    (path, SharedHandle::new(file.into()))
}

/// `ring`, sealed, or a failure naming that it was not.
fn sealed(ring: Ring) -> IoRing<Vec<u8>, usize, Owned> {
    ring.into_owned()
        .unwrap_or_else(|_| panic!("the ring has nothing in flight it holds nothing for"))
}

#[test]
fn a_ring_with_a_registration_in_flight_is_handed_back_until_it_is_popped() {
    let mut ring = Ring::with_inventory(16, 16).expect("create ring");
    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_buffers(vec![vec![0_u8; 256]])
        .expect("queue the registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");

    // The registration is in flight, and the ring holds nothing for it.
    let Err(mut ring) = ring.into_owned() else {
        panic!("sealed with a registration in flight");
    };
    assert_eq!(ring.outstanding(), 1);
    assert_eq!(ring.held(), 0);

    let (completion, held) = ring
        .pop_within(POP_BOUND)
        .expect("pop the registration")
        .expect("it completes within the bound");
    assert!(held.is_none(), "an open ring says it held nothing for it");
    pending
        .claim_if(&completion)
        .expect("the completion is the registration's")
        .expect("the registration succeeded");

    assert!(
        ring.into_owned().is_ok(),
        "nothing is in flight now, so it seals"
    );
}

#[test]
fn an_owned_ring_pops_what_it_held_without_a_case_of_nothing() {
    let (_path, shared) = fixture("flat", 7);
    let mut ring = sealed(Ring::with_inventory(16, 16).expect("create ring"));

    let mut batch = Batch::new(&mut ring);
    batch
        .read_owned(&shared, vec![0_u8; LEN], 11, 0, PushOptions::new())
        .expect("queue a read");
    batch
        .flush_owned(&shared, 12, FlushCoverage::Unordered, FlushMode::Default)
        .expect("queue a flush");
    batch.submit_and_wait(2, 5_000).expect("submit and wait");

    let mut seen = Vec::new();
    while seen.len() < 2 {
        let (completion, payload, extra) = ring
            .pop_within(POP_BOUND)
            .expect("pop")
            .expect("a completion arrives within the bound");
        seen.push((extra, payload, completion.result().is_ok()));
    }
    seen.sort_by_key(|(extra, ..)| *extra);
    assert_eq!(
        seen,
        [(11, Some(vec![7_u8; LEN]), true), (12, None, true),],
        "the read's buffer and sidecar; the flush's sidecar and no buffer"
    );
}

#[test]
fn a_registration_made_before_sealing_is_usable_after() {
    let (_path, shared) = fixture("registered", 9);
    let mut ring = Ring::with_inventory(16, 16).expect("create ring");
    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_buffers(vec![vec![0_u8; LEN]])
        .expect("queue the registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let (completion, _held) = ring
        .pop_within(POP_BOUND)
        .expect("pop")
        .expect("completes within the bound");
    let mut registered = pending
        .claim_if(&completion)
        .expect("the registration's completion")
        .expect("the registration succeeded");
    let mut ring = sealed(ring);

    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: SPAN,
    };
    let mut batch = Batch::new(&mut ring);
    batch
        .read_registered_owned(&shared, &registered, span, 5, 0, PushOptions::new())
        .expect("a registered read through the sealed ring");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let (completion, payload, extra) = ring
        .pop_within(POP_BOUND)
        .expect("pop")
        .expect("completes within the bound");
    assert_eq!(completion.result().expect("read succeeded"), LEN);
    assert_eq!(
        (payload, extra),
        (None, 5),
        "no buffer of its own, and its sidecar"
    );
    assert_eq!(registered.get(0).expect("buffer 0"), &vec![9_u8; LEN]);
}
