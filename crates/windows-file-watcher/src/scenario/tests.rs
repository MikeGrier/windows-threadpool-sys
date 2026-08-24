// Copyright (c) 2026 Mike Grier
//! Unit tests for the path-confinement check a persisted scenario's operations
//! must pass before any of them touch a real filesystem call.

use std::path::PathBuf;

use super::{HarnessOutcome, Operation, validate_barriers, validate_confined, validate_paths};

#[test]
fn a_plain_relative_path_is_confined() {
    assert!(validate_confined(&PathBuf::from("a/b.txt")).is_ok());
    assert!(validate_confined(&PathBuf::from("")).is_ok());
}

#[test]
fn an_absolute_path_is_rejected() {
    assert!(validate_confined(&PathBuf::from(r"C:\Windows\System32")).is_err());
    assert!(validate_confined(&PathBuf::from(r"\escaped")).is_err());
}

#[test]
fn a_parent_dir_component_is_rejected() {
    assert!(validate_confined(&PathBuf::from("../escaped.txt")).is_err());
    assert!(validate_confined(&PathBuf::from("a/../../escaped.txt")).is_err());
}

#[test]
fn rename_validates_both_sides() {
    let confined = Operation::Rename {
        from: PathBuf::from("a.txt"),
        to: PathBuf::from("b.txt"),
    };
    assert!(validate_paths(std::slice::from_ref(&confined)).is_ok());

    let escapes_from = Operation::Rename {
        from: PathBuf::from("../escaped.txt"),
        to: PathBuf::from("b.txt"),
    };
    assert!(validate_paths(std::slice::from_ref(&escapes_from)).is_err());

    let escapes_to = Operation::Rename {
        from: PathBuf::from("a.txt"),
        to: PathBuf::from(r"C:\escaped.txt"),
    };
    assert!(validate_paths(std::slice::from_ref(&escapes_to)).is_err());
}

#[test]
fn a_path_nested_inside_repeat_is_validated() {
    let operations = vec![Operation::Repeat {
        count: 3,
        pattern: vec![Operation::CreateFile {
            path: PathBuf::from("../escaped.txt"),
        }],
    }];
    assert!(validate_paths(&operations).is_err());
}

#[test]
fn a_path_nested_inside_a_concurrent_branch_is_validated() {
    let operations = vec![Operation::Concurrent {
        branches: vec![
            vec![Operation::CreateFile {
                path: PathBuf::from("fine.txt"),
            }],
            vec![Operation::RemoveDir {
                path: PathBuf::from("../../escaped"),
            }],
        ],
    }];
    assert!(validate_paths(&operations).is_err());
}

#[test]
fn subscribe_and_hold_open_paths_are_validated() {
    let subscribe = Operation::Subscribe {
        session: "s".to_string(),
        watch: "w".to_string(),
        path: PathBuf::from("../escaped"),
        subtree: true,
    };
    assert!(validate_paths(std::slice::from_ref(&subscribe)).is_err());

    let hold_open = Operation::HoldOpen {
        path: PathBuf::from(r"C:\escaped.txt"),
        duration: std::time::Duration::from_millis(1),
        ready_barrier: None,
    };
    assert!(validate_paths(std::slice::from_ref(&hold_open)).is_err());
}

#[test]
fn operations_without_paths_are_always_valid() {
    let operations = vec![
        Operation::Wait {
            duration: std::time::Duration::from_millis(1),
        },
        Operation::OpenSession {
            name: "s".to_string(),
        },
        Operation::CloseSession {
            name: "s".to_string(),
        },
    ];
    assert!(validate_paths(&operations).is_ok());
}

#[test]
fn harness_outcome_total_changes_whenever_any_tally_does() {
    // PR #20 review response: `total` previously omitted `volume_changes`,
    // so a lone `VolumeChanged` notification looked like no activity at all
    // to the harness's quiet-period loop, which could declare the stream
    // settled before the scenario actually was.
    let mut outcome = HarnessOutcome::default();
    assert_eq!(outcome.total(), 0);
    outcome.volume_changes = 1;
    assert_eq!(outcome.total(), 1);
}

#[test]
fn a_barrier_used_once_is_rejected() {
    let operations = vec![Operation::Barrier {
        name: "lonely".to_string(),
    }];
    assert!(validate_barriers(&operations).is_err());
}

#[test]
fn a_barrier_used_three_times_is_rejected() {
    let operations = vec![
        Operation::Barrier {
            name: "crowded".to_string(),
        },
        Operation::Barrier {
            name: "crowded".to_string(),
        },
        Operation::Barrier {
            name: "crowded".to_string(),
        },
    ];
    assert!(validate_barriers(&operations).is_err());
}

#[test]
fn a_barrier_used_exactly_twice_is_accepted() {
    let operations = vec![
        Operation::Barrier {
            name: "paired".to_string(),
        },
        Operation::HoldOpen {
            path: PathBuf::from("f.txt"),
            duration: std::time::Duration::from_millis(1),
            ready_barrier: Some("paired".to_string()),
        },
    ];
    assert!(validate_barriers(&operations).is_ok());
}

#[test]
fn a_barrier_pair_nested_inside_repeat_and_concurrent_is_still_counted() {
    let operations = vec![Operation::Repeat {
        count: 1,
        pattern: vec![Operation::Concurrent {
            branches: vec![
                vec![Operation::Barrier {
                    name: "nested".to_string(),
                }],
                vec![Operation::Barrier {
                    name: "nested".to_string(),
                }],
            ],
        }],
    }];
    assert!(validate_barriers(&operations).is_ok());
}

#[test]
fn a_barrier_name_reused_across_two_sequential_rounds_is_accepted() {
    // PR #20 review response: `DeadlineBarrier` resets after each round, so
    // reusing a name for a second, unrelated rendezvous pair later in the
    // same scenario is legitimate -- four uses total, not an unpaired
    // leftover -- and must not be rejected as if it were malformed.
    let operations = vec![
        Operation::Barrier {
            name: "reused".to_string(),
        },
        Operation::Barrier {
            name: "reused".to_string(),
        },
        Operation::Barrier {
            name: "reused".to_string(),
        },
        Operation::Barrier {
            name: "reused".to_string(),
        },
    ];
    assert!(validate_barriers(&operations).is_ok());
}
