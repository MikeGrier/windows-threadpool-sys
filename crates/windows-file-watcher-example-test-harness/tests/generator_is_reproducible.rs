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

#[test]
fn every_watch_is_established_before_it_delivers_data() {
    // A legality property the generator promises: the first notification for any
    // watch is its `Completion { Subscribed }` (schedule docs:
    // establishment-before-data).
    let schedule = Generator::with_config(GeneratorConfig {
        watches: 4,
        ..GeneratorConfig::default()
    })
    .generate(99);

    let mut seen: BTreeMap<u64, ()> = BTreeMap::new();
    for step in &schedule.steps {
        let watch = step.watch();
        if seen.insert(watch, ()).is_none() {
            assert!(
                matches!(
                    step,
                    NotificationSpec::Completion {
                        outcome: OutcomeSpec::Subscribed,
                        ..
                    }
                ),
                "watch {watch}'s first notification must be Completion {{ Subscribed }}, was {step:?}"
            );
        }
    }
    assert_eq!(seen.len(), 4, "all four watches should appear");
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
        "expected at least one VolumeChanged across 50 seeds with interactive_percent: 100"
    );
}
