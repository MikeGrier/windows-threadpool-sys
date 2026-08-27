// Copyright (c) 2026 Mike Grier
//! Integration test (M1.5): drive the example handler through a scripted
//! schedule, exactly as a downstream consumer would use the harness.

#![cfg(windows)]

use windows_file_watcher_example_test_harness::{
    ChangeKindSpec, ChangeSpec, DesyncCauseSpec, NameSpec, NotificationSpec, OutcomeSpec, Schedule,
    drive, example_handler::PresenceTracker,
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
                    kind: ChangeKindSpec::RenamedOldName,
                    name: "report.tmp".into(),
                },
                ChangeSpec {
                    kind: ChangeKindSpec::RenamedNewName,
                    name: "report.csv".into(),
                },
            ],
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

#[test]
fn a_lone_utf16_surrogate_name_round_trips_through_json_and_drives() {
    // A lone surrogate is a legal RelativeName (file-watcher D-83) but not
    // representable as a Rust String at all, so NameSpec::Units -- not
    // NameSpec::Text -- is the only way to carry it (schedule docs D-7's
    // fidelity claim).
    let mut schedule = Schedule::new();
    schedule.push(NotificationSpec::Batch {
        watch: 1,
        changes: vec![ChangeSpec {
            kind: ChangeKindSpec::Added,
            name: NameSpec::Units(vec![0xD800]),
        }],
    });

    let json = serde_json::to_string(&schedule).expect("serialize");
    let loaded: Schedule = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(loaded, schedule);

    // Also drives cleanly: the handler's lossy string conversion tolerates
    // the unpaired surrogate rather than panicking.
    let mut handler = PresenceTracker::new();
    drive(&loaded, &mut handler);
    assert_eq!(handler.present().len(), 1);
}
