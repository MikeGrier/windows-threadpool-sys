// Copyright (c) 2026 Mike Grier
//! Unit tests for the path-confinement check a persisted scenario's operations
//! must pass before any of them touch a real filesystem call.

use std::path::PathBuf;

use super::{Operation, validate_confined, validate_paths};

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
