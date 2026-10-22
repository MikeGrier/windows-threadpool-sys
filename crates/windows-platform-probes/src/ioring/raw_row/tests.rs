// Copyright (c) Mike Grier.
//! Tests for the `IoRing` probe's row.

use super::*;

fn registration() -> IoRingSupport<RegistrationObservation> {
    IoRingSupport::Measured(RegistrationObservation {
        index_one_usable_after_second: false,
        index_zero_usable_after_second: true,
    })
}

fn agnosticism(result_code: i32) -> IoRingSupport<ThreadAgnosticism> {
    IoRingSupport::Measured(ThreadAgnosticism {
        pending_at_submitter_exit: true,
        result_code,
        filled: false,
    })
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_build() {
    let row = parsed(&raw_row(&registration(), &agnosticism(0), "v1 abc [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-ioring");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
    assert!(row.get("declined").is_some_and(serde_json::Value::is_null));
}

#[test]
fn both_measurements_are_published_as_observed() {
    let row = parsed(&raw_row(&registration(), &agnosticism(0x0012_3456), "b"));
    assert_eq!(
        row["registration"],
        serde_json::json!({"index_zero_usable_after_second": true, "index_one_usable_after_second": false})
    );
    assert_eq!(
        row["thread_agnosticism"],
        serde_json::json!({"pending_at_submitter_exit": true, "result_code": "0x00123456", "filled": false})
    );
}

#[test]
fn a_failing_result_code_keeps_its_bits() {
    // An HRESULT failure has the high bit set; as an `i32` it is negative, and
    // the row must publish the code a reader would look up, not a sign.
    let code = i32::from_ne_bytes(0x8007_0005_u32.to_ne_bytes());
    let row = parsed(&raw_row(&registration(), &agnosticism(code), "b"));
    assert_eq!(row["thread_agnosticism"]["result_code"], "0x80070005");
}

#[test]
fn a_measurement_that_found_no_ring_is_null_rather_than_absent() {
    let row = parsed(&raw_row(
        &IoRingSupport::Unavailable,
        &IoRingSupport::Unavailable,
        "b",
    ));
    for key in ["registration", "thread_agnosticism"] {
        assert!(
            row.get(key).is_some_and(serde_json::Value::is_null),
            "{key}"
        );
    }
}

#[test]
fn a_declined_run_says_why_and_publishes_no_measurement() {
    let row = parsed(&declined_row(NO_USABLE_IORING, "b"));
    assert_eq!(row["declined"], NO_USABLE_IORING);
    for absent in ["registration", "thread_agnosticism"] {
        assert!(row.get(absent).is_none(), "{absent}");
    }
}
