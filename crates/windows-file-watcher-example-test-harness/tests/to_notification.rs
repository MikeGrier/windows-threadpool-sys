// Copyright (c) 2026 Mike Grier
//! Table-driven coverage of `NotificationSpec::to_notification` (PR #42
//! review): the harness wire-format boundary, exercised for every
//! `NotificationSpec` arm and every nested enum -- both `NameSpec` forms, all
//! `ChangeKindSpec` variants, every `DesyncCauseSpec`/`OutcomeSpec`/
//! `OpenFailureSpec`/`WatchModeSpec`/`FaultOperationSpec` variant, and both
//! `FailureCodeSpec` currencies.

#![cfg(windows)]

use windows_file_watcher::{
    Change, ChangeKind, DesyncCause, FailureCode, FaultDetail, FaultOperation, Notification,
    OpenFailure, Outcome, RelativeName, VolumeIdentity, WatchId, WatchMode,
};
use windows_file_watcher_example_test_harness::{
    ChangeKindSpec, ChangeSpec, DesyncCauseSpec, FailureCodeSpec, FaultDetailSpec,
    FaultOperationSpec, NameSpec, NotificationSpec, OpenFailureSpec, OutcomeSpec, VolumeSpec,
    WatchModeSpec,
};

fn watch(raw: u64) -> WatchId {
    WatchId::from_raw(raw)
}

/// Protocol identities used as representative test data below, named per the
/// repo's no-bare-numeric-constants rule. **Changing any of these values is a
/// breaking change to the test data itself** (each is an identity this
/// assertion table depends on matching a real Win32/HRESULT/action-code
/// meaning, not an arbitrary number).
mod protocol {
    /// A `FILE_ACTION_*` code file-watcher does not recognise, preserved
    /// verbatim as `ChangeKind::Unknown`.
    pub const UNKNOWN_ACTION_CODE: u32 = 99;
    /// `WinError` `ERROR_FILE_NOT_FOUND`.
    pub const ERROR_FILE_NOT_FOUND: u32 = 2;
    /// `WinError` `ERROR_NOT_SUPPORTED`.
    pub const ERROR_NOT_SUPPORTED: u32 = 50;
    /// `WinError` `ERROR_INVALID_NAME`.
    pub const ERROR_INVALID_NAME: u32 = 123;
    /// The `E_ACCESSDENIED` `HRESULT`.
    pub const E_ACCESSDENIED: i32 = 0x8007_0005u32 as i32;
}

#[test]
fn every_notification_spec_case_converts_to_the_intended_notification() {
    let cases: Vec<(&str, NotificationSpec, Notification)> = vec![
        (
            "Batch: every ChangeKindSpec, both NameSpec forms",
            NotificationSpec::Batch {
                watch: 1,
                changes: vec![
                    ChangeSpec {
                        kind: ChangeKindSpec::Added,
                        name: NameSpec::Text("added.txt".to_string()),
                    },
                    ChangeSpec {
                        kind: ChangeKindSpec::Removed,
                        name: NameSpec::Units(vec![0x72, 0x65, 0x6D]),
                    },
                    ChangeSpec {
                        kind: ChangeKindSpec::Modified,
                        name: NameSpec::Text("modified.txt".to_string()),
                    },
                    ChangeSpec {
                        kind: ChangeKindSpec::RenamedOldName,
                        name: NameSpec::Text("old.txt".to_string()),
                    },
                    ChangeSpec {
                        kind: ChangeKindSpec::RenamedNewName,
                        name: NameSpec::Text("new.txt".to_string()),
                    },
                    ChangeSpec {
                        kind: ChangeKindSpec::Unknown(protocol::UNKNOWN_ACTION_CODE),
                        name: NameSpec::Units(vec![0xD800]),
                    },
                ],
            },
            Notification::Batch {
                watch: watch(1),
                changes: vec![
                    Change {
                        kind: ChangeKind::Added,
                        name: RelativeName::for_test("added.txt"),
                    },
                    Change {
                        kind: ChangeKind::Removed,
                        name: RelativeName::for_test_units(&[0x72, 0x65, 0x6D]),
                    },
                    Change {
                        kind: ChangeKind::Modified,
                        name: RelativeName::for_test("modified.txt"),
                    },
                    Change {
                        kind: ChangeKind::RenamedOldName,
                        name: RelativeName::for_test("old.txt"),
                    },
                    Change {
                        kind: ChangeKind::RenamedNewName,
                        name: RelativeName::for_test("new.txt"),
                    },
                    Change {
                        kind: ChangeKind::Unknown(protocol::UNKNOWN_ACTION_CODE),
                        name: RelativeName::for_test_units(&[0xD800]),
                    },
                ],
            },
        ),
        (
            "Desync: Overflow",
            NotificationSpec::Desync {
                watch: 2,
                cause: DesyncCauseSpec::Overflow,
            },
            Notification::Desync {
                watch: watch(2),
                cause: DesyncCause::Overflow,
            },
        ),
        (
            "Desync: QueueFull",
            NotificationSpec::Desync {
                watch: 2,
                cause: DesyncCauseSpec::QueueFull,
            },
            Notification::Desync {
                watch: watch(2),
                cause: DesyncCause::QueueFull,
            },
        ),
        (
            "Desync: Coarse",
            NotificationSpec::Desync {
                watch: 2,
                cause: DesyncCauseSpec::Coarse,
            },
            Notification::Desync {
                watch: watch(2),
                cause: DesyncCause::Coarse,
            },
        ),
        (
            "Desync: Reestablished",
            NotificationSpec::Desync {
                watch: 2,
                cause: DesyncCauseSpec::Reestablished,
            },
            Notification::Desync {
                watch: watch(2),
                cause: DesyncCause::Reestablished,
            },
        ),
        (
            "Desync: Stopped",
            NotificationSpec::Desync {
                watch: 2,
                cause: DesyncCauseSpec::Stopped,
            },
            Notification::Desync {
                watch: watch(2),
                cause: DesyncCause::Stopped,
            },
        ),
        (
            "Completion: Subscribed",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Subscribed,
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Subscribed,
            },
        ),
        (
            "Completion: Establishing",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Establishing,
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Establishing,
            },
        ),
        (
            "Completion: Cancelled",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Cancelled,
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Cancelled,
            },
        ),
        (
            "Completion: Failed { NotFound, Win32 }",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Failed {
                    detail: FaultDetailSpec {
                        failure: OpenFailureSpec::NotFound,
                        code: FailureCodeSpec::Win32(protocol::ERROR_FILE_NOT_FOUND),
                    },
                },
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Failed {
                    detail: FaultDetail {
                        failure: OpenFailure::NotFound,
                        code: FailureCode::Win32(protocol::ERROR_FILE_NOT_FOUND),
                    },
                },
            },
        ),
        (
            "Completion: Failed { NotADirectory, HResult }",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Failed {
                    detail: FaultDetailSpec {
                        failure: OpenFailureSpec::NotADirectory,
                        code: FailureCodeSpec::HResult(-1),
                    },
                },
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Failed {
                    detail: FaultDetail {
                        failure: OpenFailure::NotADirectory,
                        code: FailureCode::HResult(-1),
                    },
                },
            },
        ),
        (
            "Completion: Failed { Unsupported, Win32 }",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Failed {
                    detail: FaultDetailSpec {
                        failure: OpenFailureSpec::Unsupported,
                        code: FailureCodeSpec::Win32(protocol::ERROR_NOT_SUPPORTED),
                    },
                },
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Failed {
                    detail: FaultDetail {
                        failure: OpenFailure::Unsupported,
                        code: FailureCode::Win32(protocol::ERROR_NOT_SUPPORTED),
                    },
                },
            },
        ),
        (
            "Completion: Failed { Retryable, HResult }",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Failed {
                    detail: FaultDetailSpec {
                        failure: OpenFailureSpec::Retryable,
                        code: FailureCodeSpec::HResult(protocol::E_ACCESSDENIED),
                    },
                },
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Failed {
                    detail: FaultDetail {
                        failure: OpenFailure::Retryable,
                        code: FailureCode::HResult(protocol::E_ACCESSDENIED),
                    },
                },
            },
        ),
        (
            "Completion: Failed { InvalidPath, Win32 }",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Failed {
                    detail: FaultDetailSpec {
                        failure: OpenFailureSpec::InvalidPath,
                        code: FailureCodeSpec::Win32(protocol::ERROR_INVALID_NAME),
                    },
                },
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Failed {
                    detail: FaultDetail {
                        failure: OpenFailure::InvalidPath,
                        code: FailureCode::Win32(protocol::ERROR_INVALID_NAME),
                    },
                },
            },
        ),
        (
            "Completion: Failed { RetryUnavailable, HResult }",
            NotificationSpec::Completion {
                watch: 3,
                outcome: OutcomeSpec::Failed {
                    detail: FaultDetailSpec {
                        failure: OpenFailureSpec::RetryUnavailable,
                        code: FailureCodeSpec::HResult(-2),
                    },
                },
            },
            Notification::Completion {
                watch: watch(3),
                outcome: Outcome::Failed {
                    detail: FaultDetail {
                        failure: OpenFailure::RetryUnavailable,
                        code: FailureCode::HResult(-2),
                    },
                },
            },
        ),
        (
            "Suspended",
            NotificationSpec::Suspended { watch: 4 },
            Notification::Suspended { watch: watch(4) },
        ),
        (
            "Resumed",
            NotificationSpec::Resumed { watch: 5 },
            Notification::Resumed { watch: watch(5) },
        ),
        (
            "Established: Detailed",
            NotificationSpec::Established {
                watch: 6,
                mode: WatchModeSpec::Detailed,
            },
            Notification::Established {
                watch: watch(6),
                mode: WatchMode::Detailed,
            },
        ),
        (
            "Established: Coarse",
            NotificationSpec::Established {
                watch: 6,
                mode: WatchModeSpec::Coarse,
            },
            Notification::Established {
                watch: watch(6),
                mode: WatchMode::Coarse,
            },
        ),
        (
            "RetryQuestion: Open",
            NotificationSpec::RetryQuestion {
                watch: 7,
                operation: FaultOperationSpec::Open,
                detail: FaultDetailSpec {
                    failure: OpenFailureSpec::NotFound,
                    code: FailureCodeSpec::Win32(protocol::ERROR_FILE_NOT_FOUND),
                },
            },
            Notification::RetryQuestion {
                watch: watch(7),
                operation: FaultOperation::Open,
                detail: FaultDetail {
                    failure: OpenFailure::NotFound,
                    code: FailureCode::Win32(protocol::ERROR_FILE_NOT_FOUND),
                },
            },
        ),
        (
            "RetryQuestion: Arm",
            NotificationSpec::RetryQuestion {
                watch: 7,
                operation: FaultOperationSpec::Arm,
                detail: FaultDetailSpec {
                    failure: OpenFailureSpec::Retryable,
                    code: FailureCodeSpec::HResult(-3),
                },
            },
            Notification::RetryQuestion {
                watch: watch(7),
                operation: FaultOperation::Arm,
                detail: FaultDetail {
                    failure: OpenFailure::Retryable,
                    code: FailureCode::HResult(-3),
                },
            },
        ),
        (
            "VolumeChanged",
            NotificationSpec::VolumeChanged {
                watch: 8,
                previous: VolumeSpec {
                    serial: 0x1111,
                    filesystem: "NTFS".to_string(),
                    label: "System".to_string(),
                },
                current: VolumeSpec {
                    serial: 0x2222,
                    filesystem: "FAT32".to_string(),
                    label: "Removable".to_string(),
                },
            },
            Notification::VolumeChanged {
                watch: watch(8),
                previous: VolumeIdentity::for_test(0x1111, "NTFS", "System"),
                current: VolumeIdentity::for_test(0x2222, "FAT32", "Removable"),
            },
        ),
    ];

    for (label, spec, expected) in cases {
        assert_eq!(spec.to_notification(), expected, "case: {label}");
    }
}
