// Copyright (c) 2026 Mike Grier
//! Integration test (M1.5): drive the example handler through a scripted
//! schedule, exactly as a downstream consumer would use the harness.

#![cfg(windows)]

use windows_file_watcher_example_test_harness::{
    ChangeKindSpec, ChangeSpec, DesyncCauseSpec, NotificationSpec, OutcomeSpec, Schedule, drive,
    example_handler::PresenceTracker,
};

#[test]
fn drives_the_example_handler_through_a_scripted_schedule() {
    let mut schedule = Schedule::new();
    schedule
        .push(NotificationSpec::Completion {
            watch: 1,
            outcome: OutcomeSpec::Subscribed,
        })
        .push(NotificationSpec::Batch {
            watch: 1,
            changes: vec![
                ChangeSpec {
                    kind: ChangeKindSpec::Added,
                    name: "report.tmp".into(),
                },
                ChangeSpec {
                    kind: ChangeKindSpec::RenamedNewName,
                    name: "report.csv".into(),
                },
            ],
        })
        .push(NotificationSpec::Batch {
            watch: 1,
            changes: vec![ChangeSpec {
                kind: ChangeKindSpec::Removed,
                name: "report.tmp".into(),
            }],
        })
        .push(NotificationSpec::Desync {
            watch: 1,
            cause: DesyncCauseSpec::Overflow,
        });

    let mut handler = PresenceTracker::new();
    drive(&schedule, &mut handler);

    assert!(handler.is_subscribed());
    assert!(handler.present().contains("report.csv"));
    assert!(!handler.present().contains("report.tmp"));
    assert_eq!(handler.rescans(), 1);
}
