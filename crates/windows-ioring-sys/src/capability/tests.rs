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

// --- decoding the raw capability struct (M18.7) ------------------------------
//
// Every assertion below was unreachable before `decode` was split out of
// `capabilities`: the query reports whatever the host supports, so a test could
// only ever see one flag combination, and M18.3's mutation run found all four
// bit operations surviving as a result.

use super::decode;
use windows_sys::Win32::Storage::FileSystem::{
    IORING_CAPABILITIES, IORING_FEATURE_FLAGS, IORING_FEATURE_SET_COMPLETION_EVENT,
    IORING_FEATURE_UM_EMULATION,
};

fn raw_with(flags: IORING_FEATURE_FLAGS) -> IORING_CAPABILITIES {
    IORING_CAPABILITIES {
        MaxVersion: 300,
        MaxSubmissionQueueSize: 64,
        MaxCompletionQueueSize: 128,
        FeatureFlags: flags,
    }
}

#[test]
fn no_feature_flags_means_no_features() {
    let decoded = decode(&raw_with(0));
    assert!(
        !decoded.supports_completion_event,
        "an empty flag mask must not report the completion event as available"
    );
    assert!(!decoded.is_emulated);
}

#[test]
fn each_feature_flag_is_read_from_its_own_bit() {
    let only_event = decode(&raw_with(IORING_FEATURE_SET_COMPLETION_EVENT));
    assert!(only_event.supports_completion_event);
    assert!(
        !only_event.is_emulated,
        "the completion-event flag must not be read as emulation"
    );

    let only_emulated = decode(&raw_with(IORING_FEATURE_UM_EMULATION));
    assert!(
        !only_emulated.supports_completion_event,
        "the emulation flag must not be read as completion-event support"
    );
    assert!(only_emulated.is_emulated);

    let both = decode(&raw_with(
        IORING_FEATURE_SET_COMPLETION_EVENT | IORING_FEATURE_UM_EMULATION,
    ));
    assert!(both.supports_completion_event);
    assert!(both.is_emulated);
}

#[test]
fn an_unknown_feature_bit_changes_neither_answer() {
    // A future Windows reporting a feature this crate does not name must not
    // be mistaken for one it does. This is the case that distinguishes a
    // masked test from a truthiness test on the whole word.
    let unknown: IORING_FEATURE_FLAGS =
        !(IORING_FEATURE_SET_COMPLETION_EVENT | IORING_FEATURE_UM_EMULATION);

    let decoded = decode(&raw_with(unknown));
    assert!(!decoded.supports_completion_event);
    assert!(!decoded.is_emulated);

    // And the named flags must still be found when they arrive alongside it.
    let with_event = decode(&raw_with(unknown | IORING_FEATURE_SET_COMPLETION_EVENT));
    assert!(with_event.supports_completion_event);
    assert!(!with_event.is_emulated);
}

#[test]
fn the_numeric_fields_pass_through_unchanged() {
    let decoded = decode(&raw_with(0));
    assert_eq!(decoded.max_version, RingVersion::from_raw(300));
    assert_eq!(decoded.max_submission_queue_size, 64);
    assert_eq!(decoded.max_completion_queue_size, 128);

    // Distinct values, so a decoder that crossed the two queue sizes would be
    // caught rather than passing on their happening to agree.
    let swapped = decode(&IORING_CAPABILITIES {
        MaxVersion: 400,
        MaxSubmissionQueueSize: 128,
        MaxCompletionQueueSize: 64,
        FeatureFlags: 0,
    });
    assert_eq!(swapped.max_version, RingVersion::from_raw(400));
    assert_eq!(swapped.max_submission_queue_size, 128);
    assert_eq!(swapped.max_completion_queue_size, 64);
}

#[test]
fn the_query_agrees_with_decoding_what_the_host_reports() {
    // Ties the pure function to the syscall path: the split must not have left
    // `capabilities()` decoding differently from `decode`.
    let queried =
        capabilities().expect("QueryIoRingCapabilities should succeed on a supported host");
    let round_tripped = decode(&IORING_CAPABILITIES {
        MaxVersion: queried.max_version.raw(),
        MaxSubmissionQueueSize: queried.max_submission_queue_size,
        MaxCompletionQueueSize: queried.max_completion_queue_size,
        FeatureFlags: if queried.supports_completion_event {
            IORING_FEATURE_SET_COMPLETION_EVENT
        } else {
            0
        } | if queried.is_emulated {
            IORING_FEATURE_UM_EMULATION
        } else {
            0
        },
    });
    assert_eq!(queried, round_tripped);
}
