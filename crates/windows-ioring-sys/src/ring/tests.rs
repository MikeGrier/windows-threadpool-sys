// Copyright (c) 2026 Mike Grier
use super::{IoRing, Op, OpSupport};
use crate::capability::{RingVersion, capabilities};

#[test]
fn op_support_starts_empty() {
    assert!(!OpSupport::default().contains(Op::Read));
    assert!(!OpSupport::default().contains(Op::Nop));
}

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
fn every_named_version_the_host_supports_creates_and_closes() {
    let caps = capabilities().expect("capabilities");
    let mut created_at_least_one = false;
    for version in [RingVersion::V1, RingVersion::V2, RingVersion::V3] {
        if version > caps.max_version {
            continue;
        }
        let ring = IoRing::with_version(version, 64, 128).expect("create at a supported version");
        assert_eq!(ring.version(), version);
        drop(ring);
        created_at_least_one = true;
    }
    assert!(
        created_at_least_one,
        "the host should support at least IORING_VERSION_1"
    );
}

#[test]
fn creating_a_ring_above_the_hosts_maximum_version_fails() {
    let caps = capabilities().expect("capabilities");
    let too_high = RingVersion::from_raw(caps.max_version.raw() + 1);
    let error =
        IoRing::with_version(too_high, 64, 128).expect_err("an unsupported version must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
}

#[test]
fn capability_reporting_never_claims_more_than_is_io_ring_op_supported_reports() {
    let ring = IoRing::new(64, 128).expect("create ring");
    for op in [
        Op::Nop,
        Op::Read,
        Op::Write,
        Op::Flush,
        Op::RegisterFiles,
        Op::RegisterBuffers,
        Op::Cancel,
    ] {
        assert_eq!(
            ring.supports(op),
            ring.supports_raw(op.code()),
            "cached support for {op:?} disagrees with a direct IsIoRingOpSupported call"
        );
    }
}

#[test]
fn nop_read_and_write_are_supported_on_any_real_ring() {
    // A sanity floor: every documented IoRing version supports at least
    // these three. If this ever fails, either the host is exotic enough to
    // need investigating, or the probe itself is broken.
    let ring = IoRing::new(64, 128).expect("create ring");
    assert!(ring.supports(Op::Nop));
    assert!(ring.supports(Op::Read));
    assert!(ring.supports(Op::Write));
}

// --- outstanding-operation accounting and rundown (M2.4) ---

#[test]
fn reserve_user_data_increments_outstanding_and_never_repeats_an_id() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    let a = ring.reserve_user_data().expect("reserve a");
    let b = ring.reserve_user_data().expect("reserve b");
    assert_ne!(a, b);
    assert_eq!(ring.outstanding(), 2);
    ring.record_completion();
    ring.record_completion();
}

#[test]
fn run_down_is_a_no_op_when_nothing_is_outstanding() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    ring.run_down().expect("run_down with nothing outstanding");
    assert_eq!(ring.outstanding(), 0);
}

#[test]
fn run_down_returns_once_a_recorded_completion_zeroes_the_count() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    ring.reserve_user_data().expect("reserve");
    assert_eq!(ring.outstanding(), 1);
    // Recording the completion up front proves run_down rechecks the count
    // rather than always performing at least one wait: it must return
    // without ever calling SubmitIoRing, or this test would hang for
    // RUN_DOWN_POLL_MS waiting on a completion that was never real.
    ring.record_completion();
    ring.run_down()
        .expect("run_down with the count already settled");
    assert_eq!(ring.outstanding(), 0);
}

#[test]
fn record_completion_saturates_rather_than_underflowing() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    assert_eq!(ring.outstanding(), 0);
    ring.record_completion();
    assert_eq!(
        ring.outstanding(),
        0,
        "recording more completions than were ever reserved must not wrap"
    );
}

#[test]
fn dropping_a_ring_with_nothing_outstanding_does_not_hang() {
    // The ordinary path: no tokens were ever minted, so Drop's run_down must
    // return immediately rather than waiting on SubmitIoRing at all.
    let ring = IoRing::new(64, 128).expect("create ring");
    drop(ring);
}
