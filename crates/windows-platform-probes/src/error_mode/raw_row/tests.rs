// Copyright (c) Mike Grier.
//! Tests for the error-mode probe's row.

use super::*;

/// Distinct values in every field, so no two can be swapped unnoticed.
fn observed() -> Observation {
    Observation {
        bits: vec![
            BitOutcome {
                bit: bits::FAIL_CRITICAL_ERRORS,
                set_ok: true,
                last_error: 0,
                read_back: 0x0001,
            },
            BitOutcome {
                bit: bits::NO_ALIGNMENT_FAULT_EXCEPT,
                set_ok: false,
                last_error: 87,
                read_back: 0x0010,
            },
        ],
        combined_read_back: 0x0020,
        process_versus_thread: ProcessVersusThread {
            process_bit: bits::FAIL_CRITICAL_ERRORS,
            process_mode: 0x0041,
            thread_mode: 0x0080,
        },
        alignment: (0x0100, 0x0104),
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_build() {
    let row = parsed(&raw_row(&observed(), "v1 abc [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-error-mode");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
}

#[test]
fn each_bit_is_published_with_the_error_only_when_the_call_failed() {
    let row = parsed(&raw_row(&observed(), "b"));
    assert_eq!(
        row["bits"],
        serde_json::json!([
            {"bit": 1, "set_ok": true, "last_error": null, "read_back": 1},
            {"bit": 4, "set_ok": false, "last_error": 87, "read_back": 16},
        ])
    );
}

#[test]
fn the_combined_call_publishes_what_it_attempted_and_what_it_read_back() {
    let row = parsed(&raw_row(&observed(), "b"));
    assert_eq!(
        row["combined"],
        serde_json::json!({"attempted": COMBINED_ATTEMPT, "read_back": 32})
    );
    // The attempt is two valid bits and one invalid one, as the module states.
    assert_eq!(COMBINED_ATTEMPT, 0x0007);
    assert_eq!(COMBINED_VALID, 0x0003);
}

#[test]
fn the_two_scopes_and_the_alignment_restore_are_published_as_read() {
    let row = parsed(&raw_row(&observed(), "b"));
    assert_eq!(
        row["process_versus_thread"],
        serde_json::json!({"process_bit": 1, "process_mode": 65, "thread_mode": 128})
    );
    assert_eq!(
        row["alignment_at_process_scope"],
        serde_json::json!({"before": 256, "after_restore": 260})
    );
}

#[test]
fn the_probed_bits_are_the_four_sem_bits_once_each() {
    let mut sorted = PROBED_BITS;
    sorted.sort_unstable();
    assert_eq!(sorted, [0x0001, 0x0002, 0x0004, 0x8000]);
}
