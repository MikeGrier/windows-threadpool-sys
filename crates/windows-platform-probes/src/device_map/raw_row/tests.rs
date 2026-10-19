// Copyright (c) Mike Grier.
//! Tests for the device-map probe's row.

use super::*;

fn finding() -> DeviceMapFinding {
    DeviceMapFinding {
        own_session: MapObservation {
            letter: "Q:".to_owned(),
            target: Some("\\??\\C:\\probe-a\0\\??\\C:\\other\0".to_owned()),
            logon_session: Some((0x0001_2345, 0)),
        },
        anonymous_session: MapObservation {
            letter: "Q:".to_owned(),
            target: None,
            logon_session: Some((0x3e6, -2)),
        },
        target: "\\??\\C:\\probe-a".to_owned(),
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_build() {
    let row = parsed(&raw_row(&finding(), "v1 abc [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-device-map");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
    assert!(row.get("declined").is_some_and(serde_json::Value::is_null));
    assert_eq!(row["target"], "\\??\\C:\\probe-a");
}

#[test]
fn each_session_publishes_every_target_and_its_logon_session() {
    let row = parsed(&raw_row(&finding(), "b"));
    assert_eq!(
        row["own_session"],
        serde_json::json!({
            "letter": "Q:",
            "targets": ["\\??\\C:\\probe-a", "\\??\\C:\\other"],
            "logon_session": "00000000:00012345",
        })
    );
}

#[test]
fn a_letter_that_resolved_to_nothing_is_null_and_a_signed_high_half_keeps_its_bits() {
    let row = parsed(&raw_row(&finding(), "b"));
    assert_eq!(
        row["anonymous_session"],
        serde_json::json!({"letter": "Q:", "targets": null, "logon_session": "fffffffe:000003e6"})
    );
}

#[test]
fn an_unread_logon_session_is_null() {
    let mut unread = finding();
    unread.own_session.logon_session = None;
    let row = parsed(&raw_row(&unread, "b"));
    assert!(
        row["own_session"]
            .get("logon_session")
            .is_some_and(serde_json::Value::is_null)
    );
}

#[test]
fn a_declined_run_says_why_and_publishes_no_sessions() {
    let row = parsed(&declined_row(NO_FREE_DRIVE_LETTER, "b"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(row["declined"], NO_FREE_DRIVE_LETTER);
    for absent in ["target", "own_session", "anonymous_session"] {
        assert!(row.get(absent).is_none(), "{absent}");
    }
}
