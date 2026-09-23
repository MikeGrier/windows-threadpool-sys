// Copyright (c) 2026 Mike Grier
//! A ring's own lifecycle, through public API only (M24.3).
//!
//! Relocated from `src/ring/tests.rs` at 9bc0350e. These open a real kernel ring,
//! which the repository's Quality rule classifies as an external boundary, so
//! `src/` was never where they belonged -- see
//! [D-49](../DESIGN-NOTES.md#d-49). They reach nothing crate-private, which is
//! what made them relocatable where the other 39 ring-opening lib tests are not.
//!
//! Pure relocation: the bodies are unchanged.

use windows_ioring_sys::{IoRing, RingVersion, capabilities};

#[test]
fn a_ring_negotiates_a_version_no_higher_than_the_hosts_maximum() {
    let ring = IoRing::new(64, 128).expect("create ring");
    let caps = capabilities().expect("capabilities");
    assert!(ring.version() <= caps.max_version);
    assert!(ring.version() <= RingVersion::HIGHEST_KNOWN);
}

#[test]
fn a_negotiated_ring_reports_its_version_back_through_get_ring_info() {
    let ring = IoRing::new(64, 128).expect("create ring");
    let info = ring.info().expect("GetIoRingInfo");
    assert_eq!(info.version, ring.version());
}

#[test]
fn run_down_is_a_no_op_when_nothing_is_outstanding() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    ring.run_down().expect("run_down with nothing outstanding");
    assert_eq!(ring.outstanding(), 0);
}

#[test]
fn dropping_a_ring_with_nothing_outstanding_does_not_hang() {
    // The ordinary path: no tokens were ever minted, so Drop's run_down must
    // return immediately rather than waiting on SubmitIoRing at all.
    let ring = IoRing::new(64, 128).expect("create ring");
    drop(ring);
}
