// Copyright (c) 2026 Mike Grier
use super::EventDelivery;
use crate::IoRing;

#[test]
fn new_succeeds_and_the_ring_stays_reachable_for_pushes() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let delivery = EventDelivery::new(ring, |_completion| {}, None).expect("wire event delivery");
    let info = delivery
        .ring()
        .lock()
        .expect("lock ring")
        .info()
        .expect("query info");
    assert!(info.submission_queue_size > 0);
}

#[test]
fn dropping_with_nothing_outstanding_does_not_hang() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let delivery = EventDelivery::new(ring, |_completion| {}, None).expect("wire event delivery");
    drop(delivery);
}
