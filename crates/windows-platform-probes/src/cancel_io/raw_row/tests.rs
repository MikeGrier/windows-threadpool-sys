// Copyright (c) Mike Grier.
//! Tests for the cancel-io probe's row.

use super::*;

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_watchdog_and_the_build() {
    let row = parsed(&raw_row(
        CancelOutcome::NotFound { code: 1168 },
        &[],
        "v1 abc [ci]",
    ));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-cancel-io");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
    assert_eq!(row["watchdog_ms"], 5000);
}

#[test]
fn the_idle_case_publishes_the_code_the_call_reported() {
    let row = parsed(&raw_row(CancelOutcome::NotFound { code: 1168 }, &[], "b"));
    assert_eq!(
        row["idle"],
        serde_json::json!({"outcome": "not_found", "code": 1168})
    );
}

#[test]
fn every_busy_attempt_is_published_in_order_including_one_the_watchdog_abandoned() {
    let row = parsed(&raw_row(
        CancelOutcome::Cancelled,
        &[
            CancelOutcome::Cancelled,
            CancelOutcome::Wedged,
            CancelOutcome::NotFound { code: 6 },
        ],
        "b",
    ));
    assert_eq!(
        row["idle"],
        serde_json::json!({"outcome": "cancelled", "code": null})
    );
    assert_eq!(
        row["busy"],
        serde_json::json!([
            {"outcome": "cancelled", "code": null},
            {"outcome": "wedged", "code": null},
            {"outcome": "not_found", "code": 6},
        ])
    );
}

#[test]
fn a_run_that_wedged_on_every_attempt_still_publishes_every_attempt() {
    let busy = [CancelOutcome::Wedged; BUSY_ATTEMPTS];
    let row = parsed(&raw_row(CancelOutcome::Wedged, &busy, "b"));
    assert_eq!(row["busy"].as_array().map(Vec::len), Some(BUSY_ATTEMPTS));
    assert_eq!(row["idle"]["outcome"], "wedged");
}
