// Copyright (c) Mike Grier.
//! Tests for the completion-port probe's row.

use super::*;

fn attempt(result_code: u32, bytes: usize, first_byte: u8) -> ReadAttempt {
    ReadAttempt {
        result_code: i32::from_ne_bytes(result_code.to_ne_bytes()),
        bytes,
        first_byte,
    }
}

/// Distinct values in every case, so no two can be swapped unnoticed.
fn finding() -> CompletionPortFinding {
    CompletionPortFinding {
        control_unassociated: attempt(0, 4096, 0xAB),
        after_iocp_association: attempt(0x8007_0057, 0, 0),
        before_late_association: attempt(0, 4095, 0xAB),
        after_late_association: attempt(0x8007_0006, 1, 2),
        port_still_works: true,
        before_threadpool_io: attempt(0, 4094, 0xAB),
        after_threadpool_io: attempt(0x8007_0005, 3, 4),
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_fixture_and_the_build() {
    let row = parsed(&raw_row(&IoRingSupport::Measured(finding()), "v1 abc [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-completion-port");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
    assert_eq!(row["fill"], FILL);
    assert_eq!(row["read_len"], FIXTURE_LEN);
    assert!(row.get("declined").is_some_and(serde_json::Value::is_null));
}

#[test]
fn every_read_case_is_published_under_its_own_name() {
    let row = parsed(&raw_row(&IoRingSupport::Measured(finding()), "b"));
    let read = |code: &str, bytes: usize, first: u8| serde_json::json!({"result_code": code, "bytes": bytes, "first_byte": first});
    assert_eq!(
        row["reads"],
        serde_json::json!({
            "control_unassociated": read("0x00000000", 4096, 0xAB),
            "after_iocp_association": read("0x80070057", 0, 0),
            "before_late_association": read("0x00000000", 4095, 0xAB),
            "after_late_association": read("0x80070006", 1, 2),
            "before_threadpool_io": read("0x00000000", 4094, 0xAB),
            "after_threadpool_io": read("0x80070005", 3, 4),
        })
    );
    assert_eq!(row["port_still_works"], true);
}

#[test]
fn a_port_that_stopped_working_is_published_as_false() {
    let mut broken = finding();
    broken.port_still_works = false;
    let row = parsed(&raw_row(&IoRingSupport::Measured(broken), "b"));
    assert_eq!(row["port_still_works"], false);
}

#[test]
fn a_host_without_a_ring_declines_and_publishes_no_reads() {
    let row = parsed(&raw_row(&IoRingSupport::Unavailable, "b"));
    assert_eq!(row["declined"], NO_USABLE_IORING);
    assert_eq!(row["fill"], FILL);
    for absent in ["reads", "port_still_works"] {
        assert!(row.get(absent).is_none(), "{absent}");
    }
}
