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
