// Copyright (c) Mike Grier.
//! Tests for the worker-context probe's row.

use super::*;

/// Distinct values in every field, so no two can be swapped unnoticed.
fn plain() -> WorkerContext {
    WorkerContext {
        has_thread_token: false,
        open_token_error: ERROR_NO_TOKEN,
        error_mode: 0x0010,
    }
}

fn impersonating() -> IdentityAsymmetry {
    IdentityAsymmetry {
        submitter: WorkerContext {
            has_thread_token: true,
            open_token_error: 0,
            error_mode: 0x0020,
        },
        worker: WorkerContext {
            has_thread_token: false,
            open_token_error: 5,
            error_mode: 0x0040,
        },
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_build() {
    let row = parsed(&raw_row(plain(), impersonating(), "v1 abc [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-worker-context");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
}

#[test]
fn the_plain_worker_is_published_as_found() {
    let row = parsed(&raw_row(plain(), impersonating(), "b"));
    assert_eq!(
        row["plain_worker"],
        serde_json::json!({"has_thread_token": false, "open_token_error": ERROR_NO_TOKEN, "error_mode": 16})
    );
}

#[test]
fn both_sides_of_the_impersonating_submission_are_published_with_the_error_only_without_a_token() {
    let row = parsed(&raw_row(plain(), impersonating(), "b"));
    assert_eq!(
        row["impersonating"],
        serde_json::json!({
            "submitter": {"has_thread_token": true, "open_token_error": null, "error_mode": 32},
            "worker": {"has_thread_token": false, "open_token_error": 5, "error_mode": 64},
        })
    );
}
