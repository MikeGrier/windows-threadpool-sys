// Copyright (c) 2026 Mike Grier
use super::{Capabilities, RingVersion, capabilities};

#[test]
fn v3_is_higher_than_v1_and_v2() {
    assert!(RingVersion::V3 > RingVersion::V2);
    assert!(RingVersion::V2 > RingVersion::V1);
}

#[test]
fn highest_known_is_v3() {
    assert_eq!(RingVersion::HIGHEST_KNOWN, RingVersion::V3);
}

#[test]
fn from_raw_round_trips_a_version_this_crate_does_not_name() {
    // The whole reason from_raw/raw exist: a version like 400, seen on a
    // real machine, that no named constant covers yet.
    let unnamed = RingVersion::from_raw(400);
    assert_eq!(unnamed.raw(), 400);
    assert!(unnamed > RingVersion::HIGHEST_KNOWN);
}

#[test]
fn capabilities_can_be_queried_without_creating_a_ring() {
    let caps = capabilities().expect("QueryIoRingCapabilities should succeed on a supported host");
    assert!(caps.max_version >= RingVersion::V1);
    assert!(caps.max_submission_queue_size > 0);
    assert!(caps.max_completion_queue_size > 0);
}

#[test]
fn capabilities_is_copy_and_comparable() {
    let a = Capabilities {
        max_version: RingVersion::V3,
        max_submission_queue_size: 64,
        max_completion_queue_size: 128,
        supports_completion_event: true,
        is_emulated: false,
    };
    let b = a;
    assert_eq!(a, b);
}
