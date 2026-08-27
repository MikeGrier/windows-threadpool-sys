// Copyright (c) 2026 Mike Grier
//! Integration mode 1: an in-process unit test.
//!
//! The simplest way to use this harness: script a [`Schedule`] by hand, drive
//! your handler, and assert on its reactions. No filesystem, no thread pool, no
//! chaos -- just your handler and a fixed, deterministic sequence of
//! notifications.
//!
//! ```text
//! cargo run --example in_process_test
//! ```

use windows_file_watcher_example_test_harness::{
    ChangeKindSpec, ChangeSpec, DesyncCauseSpec, NotificationSpec, OutcomeSpec, Schedule, drive,
    example_handler::PresenceTracker,
};

fn main() {
    // Build a schedule describing exactly the traffic you want to test against.
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
                    name: "draft.tmp".into(),
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

    // Drive it: no filesystem, no thread pool, fully deterministic.
    let mut handler = PresenceTracker::new();
    drive(&schedule, &mut handler);

    // Assert on your handler's reactions -- this is the whole point.
    assert!(handler.is_subscribed());
    assert!(handler.present().contains("report.csv"));
    assert_eq!(handler.rescans(), 1);

    println!("in-process test passed: {handler:?}");
}
