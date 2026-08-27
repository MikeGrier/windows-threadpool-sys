// Copyright (c) 2026 Mike Grier
//! M2.3: the generator is reproducible, and the schedules it produces respect
//! the legal-envelope rules the generator promises (crate DESIGN-NOTES D-5).

#![cfg(windows)]

use std::collections::BTreeMap;

use windows_file_watcher_example_test_harness::{
    Generator, GeneratorConfig, NotificationSpec, OutcomeSpec,
};

#[test]
fn a_fixed_seed_generates_an_identical_schedule() {
    let generator = Generator::new();
    let first = generator.generate(0x0ABC_D123_4567_89AB);
    let second = generator.generate(0x0ABC_D123_4567_89AB);
    assert_eq!(
        first, second,
        "the same seed must produce the same schedule"
    );
    assert!(!first.is_empty());
}

#[test]
fn different_seeds_produce_different_schedules() {
    let generator = Generator::new();
    assert_ne!(generator.generate(1), generator.generate(2));
}

/// Group a schedule's steps by watch, preserving each watch's relative order
/// (per-watch order is what carries sequencing meaning; cross-watch order is
/// free -- schedule docs).
fn by_watch(
    schedule: &windows_file_watcher_example_test_harness::Schedule,
) -> BTreeMap<u64, Vec<&NotificationSpec>> {
    let mut grouped: BTreeMap<u64, Vec<&NotificationSpec>> = BTreeMap::new();
    for step in &schedule.steps {
        grouped.entry(step.watch()).or_default().push(step);
    }
    grouped
}

/// A handful of seeds and shapes, not just one -- the review that found the
/// original bugs here was explicit that a single seed/configuration is not
/// enough coverage for a "every schedule is legal" claim.
fn sample_configs() -> Vec<GeneratorConfig> {
    vec![
        GeneratorConfig::default(),
        GeneratorConfig {
            watches: 6,
            steps_per_watch: 30,
            liveness_percent: 100,
            interactive_percent: 100,
            cancel_percent: 60,
            ..GeneratorConfig::default()
        },
        GeneratorConfig {
            watches: 2,
            steps_per_watch: 25,
            liveness_percent: 0,
            interactive_percent: 100,
            question_percent: 100,
            ..GeneratorConfig::default()
        },
        GeneratorConfig {
            watches: 2,
            steps_per_watch: 25,
            liveness_percent: 100,
            interactive_percent: 0,
            ..GeneratorConfig::default()
        },
    ]
}

#[test]
fn first_two_notifications_of_a_liveness_watch_are_established_then_subscribed() {
    // windows-file-watcher sends the initial Established from inside route
    // establishment, and only afterward turns the result into the Completion
    // its caller reports (schedule docs: establishment precedes data, and
    // Established precedes Completion for a liveness watch).
    for config in sample_configs() {
        let generator = Generator::with_config(config);
        for seed in 0..10 {
            let schedule = generator.generate(seed);
            for (watch, steps) in by_watch(&schedule) {
                match steps.first() {
                    Some(NotificationSpec::Established { .. }) => {
                        assert!(
                            matches!(
                                steps.get(1),
                                Some(NotificationSpec::Completion {
                                    outcome: OutcomeSpec::Subscribed,
                                    ..
                                })
                            ),
                            "seed {seed}, watch {watch}: Established must be immediately \
                             followed by Completion {{ Subscribed }}, got {:?}",
                            steps.get(1)
                        );
                    }
                    Some(NotificationSpec::Completion {
                        outcome: OutcomeSpec::Subscribed,
                        ..
                    }) => {}
                    other => panic!(
                        "seed {seed}, watch {watch}: first notification must be Established \
                         or Completion {{ Subscribed }}, got {other:?}"
                    ),
                }
            }
        }
    }
}

#[test]
fn every_question_is_immediately_resolved_with_a_reestablished_desync() {
    // A RetryQuestion/VolumeChanged only ever arises from inside a fault, and
    // this generator's fault-recovery event pushes the resolution
    // contiguously, so no other notification for the same watch can land
    // between a question and its resolution (schedule docs: a fault and its
    // resolution are one unit). `RetryMode::Interactive` and
    // `VolumeChangePolicy::Confirm` are independent options, so a single
    // recovery may surface a `RetryQuestion` followed by a `VolumeChanged` (or
    // just one, or neither) before the resolution -- so the step immediately
    // after a question is either the other kind of question or the
    // resolution itself, never anything else.
    for config in sample_configs() {
        let generator = Generator::with_config(config);
        for seed in 0..10 {
            let schedule = generator.generate(seed);
            for (watch, steps) in by_watch(&schedule) {
                for (index, step) in steps.iter().enumerate() {
                    let is_question = matches!(
                        step,
                        NotificationSpec::RetryQuestion { .. }
                            | NotificationSpec::VolumeChanged { .. }
                    );
                    if is_question {
                        assert!(
                            matches!(
                                steps.get(index + 1),
                                Some(NotificationSpec::RetryQuestion { .. })
                                    | Some(NotificationSpec::VolumeChanged { .. })
                            ) || matches!(
                                steps.get(index + 1),
                                Some(NotificationSpec::Desync { cause, .. })
                                    if matches!(
                                        cause,
                                        windows_file_watcher_example_test_harness::DesyncCauseSpec::Reestablished
                                    )
                            ),
                            "seed {seed}, watch {watch}: a question must be immediately \
                             followed by another question or resolved with \
                             Desync {{ Reestablished }}, got {:?}",
                            steps.get(index + 1)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn every_resumed_is_immediately_followed_by_established() {
    // resolve_fault_success never sends Resumed without Established, or vice
    // versa -- they are always together (schedule docs).
    for config in sample_configs() {
        let generator = Generator::with_config(config);
        for seed in 0..10 {
            let schedule = generator.generate(seed);
            for (watch, steps) in by_watch(&schedule) {
                for (index, step) in steps.iter().enumerate() {
                    if matches!(step, NotificationSpec::Resumed { .. }) {
                        assert!(
                            matches!(
                                steps.get(index + 1),
                                Some(NotificationSpec::Established { .. })
                            ),
                            "seed {seed}, watch {watch}: Resumed must be immediately followed \
                             by Established, got {:?}",
                            steps.get(index + 1)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn cancelled_is_always_the_last_notification_for_its_watch() {
    for config in sample_configs() {
        let generator = Generator::with_config(config);
        for seed in 0..10 {
            let schedule = generator.generate(seed);
            for (watch, steps) in by_watch(&schedule) {
                if let Some(position) = steps.iter().position(|step| {
                    matches!(
                        step,
                        NotificationSpec::Completion {
                            outcome: OutcomeSpec::Cancelled,
                            ..
                        }
                    )
                }) {
                    assert_eq!(
                        position,
                        steps.len() - 1,
                        "seed {seed}, watch {watch}: Cancelled must be the last notification \
                         for its watch"
                    );
                }
            }
        }
    }
}

#[test]
fn every_generated_volume_changed_has_a_distinct_previous_and_current_serial() {
    // Regression test (PR #42 review): windows-file-watcher only emits
    // VolumeChanged when the volume identity actually differs (D-78), and
    // identity compares by serial alone (D-50). An equal previous/current
    // serial would be an impossible, illegal notification.
    let generator = Generator::with_config(GeneratorConfig {
        watches: 4,
        steps_per_watch: 40,
        interactive_percent: 100,
        question_percent: 100,
        ..GeneratorConfig::default()
    });

    let mut checked = 0;
    for seed in 0..50 {
        for step in &generator.generate(seed).steps {
            if let NotificationSpec::VolumeChanged {
                previous, current, ..
            } = step
            {
                assert_ne!(
                    previous.serial, current.serial,
                    "seed {seed}: a VolumeChanged with equal serials is not a legal schedule"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked > 0,
        "expected at least one VolumeChanged across 50 seeds with interactive_percent/question_percent: 100"
    );
}
