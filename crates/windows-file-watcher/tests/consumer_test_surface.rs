// Copyright (c) 2026 Mike Grier
//! Integration test for the consumer test surface (M13.6).
//!
//! This test crate is an ordinary downstream consumer: it can reach only the
//! crate's public API plus the `test-util` feature, never any `pub(crate)`
//! item. It drives a scripted sequence covering every `Notification` variant --
//! including the two `test-util`-only gap-filled types (`RelativeName` inside a
//! `Change`, and `VolumeIdentity` inside a `VolumeChanged`) -- and asserts
//! deterministic receipt through a `Receiver` it feeds itself, with no
//! filesystem and no thread pool.

#![cfg(windows)]

use windows_file_watcher::{
    Change, ChangeKind, DEFAULT_BOUND, Delivery, DesyncCause, FailureCode, FaultDetail,
    FaultOperation, Notification, OpenFailure, Outcome, RelativeName, VolumeIdentity, WatchId,
    WatchMode, channel_with_bound,
};

/// The real Win32 error code paired with `OpenFailure::Retryable` below,
/// named per the repo's no-bare-numeric-constants rule.
const ERROR_ACCESS_DENIED: u32 = 5;

/// One of every `Notification` variant, in a fixed order, tagged to `watch`.
fn every_variant(watch: WatchId) -> Vec<Notification> {
    vec![
        Notification::Completion {
            watch,
            outcome: Outcome::Subscribed,
        },
        Notification::Established {
            watch,
            mode: WatchMode::Detailed,
        },
        Notification::Batch {
            watch,
            changes: vec![
                Change {
                    kind: ChangeKind::Added,
                    name: RelativeName::for_test("a.txt"),
                },
                Change {
                    kind: ChangeKind::Modified,
                    name: RelativeName::for_test("b.log"),
                },
            ],
        },
        Notification::Desync {
            watch,
            cause: DesyncCause::Overflow,
        },
        Notification::Suspended { watch },
        Notification::Resumed { watch },
        Notification::RetryQuestion {
            watch,
            operation: FaultOperation::Arm,
            detail: FaultDetail {
                failure: OpenFailure::Retryable,
                code: FailureCode::Win32(ERROR_ACCESS_DENIED),
            },
        },
        Notification::VolumeChanged {
            watch,
            previous: VolumeIdentity::for_test(1, "NTFS", "A"),
            current: VolumeIdentity::for_test(2, "ReFS", "B"),
        },
    ]
}

#[test]
fn every_notification_variant_is_received_in_submission_order() {
    let (sender, receiver) = channel_with_bound(DEFAULT_BOUND);
    let watch = WatchId::from_raw(7);

    let sent = every_variant(watch);
    for notification in &sent {
        assert!(
            matches!(sender.send(notification.clone()), Delivery::Queued),
            "a spacious queue accepts every notification"
        );
    }

    let mut received = Vec::new();
    while let Some(notification) = receiver.try_recv() {
        received.push(notification);
    }

    assert_eq!(received, sent, "every variant is delivered, in order");
}

#[test]
fn a_drained_receiver_reports_empty_then_disconnected() {
    let (sender, receiver) = channel_with_bound(DEFAULT_BOUND);
    let watch = WatchId::from_raw(1);
    let _ = sender.send(Notification::Desync {
        watch,
        cause: DesyncCause::Coarse,
    });

    assert!(matches!(
        receiver.try_recv(),
        Some(Notification::Desync { .. })
    ));
    assert!(receiver.try_recv().is_none(), "nothing left to drain");

    drop(sender);
    // With every sender gone and the queue empty, a blocking recv returns None
    // rather than hanging -- how a consumer's drain loop ends on teardown.
    assert!(receiver.recv().is_none());
}
