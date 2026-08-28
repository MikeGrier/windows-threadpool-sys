// Copyright (c) 2026 Mike Grier
//! M4.2: generate -> find a pathology -> serialize -> deserialize -> replay ->
//! the same pathology, deterministically. This is the payoff: a pathology found
//! once becomes a reproducible regression, carried as plain JSON.

#![cfg(windows)]

use windows_file_watcher::Notification;
use windows_file_watcher_example_test_harness::{
    Generator, GeneratorConfig, Handler, Recording, run,
};

/// Panics the first time it sees a `Desync` -- reliably reachable from a
/// generated schedule (the generator emits loss desyncs by design), so this
/// test does not depend on a lucky seed to find a pathology quickly.
struct PanicsOnDesync;
impl Handler for PanicsOnDesync {
    fn on(&mut self, notification: &Notification) {
        assert!(
            !matches!(notification, Notification::Desync { .. }),
            "handler cannot cope with a desync"
        );
    }
}

#[test]
fn a_captured_pathology_reproduces_identically_after_a_json_round_trip() {
    let generator = Generator::with_config(GeneratorConfig {
        watches: 1,
        steps_per_watch: 8,
        ..GeneratorConfig::default()
    });

    // Find a seed that actually trips the oracle (the generator also emits
    // batches with no desync at all, so not every seed is a hit).
    let (seed, schedule, outcome) = (0..100)
        .find_map(|seed| {
            let schedule = generator.generate(seed);
            let outcome = run(&schedule, &mut PanicsOnDesync);
            outcome
                .pathology()
                .is_some()
                .then_some((seed, schedule, outcome))
        })
        .expect("at least one of the first 100 seeds should trip the panic oracle");

    let recording = Recording::new(seed, schedule, outcome.clone());
    let json = recording.to_json().expect("serialize");
    let loaded = Recording::from_json(&json).expect("deserialize");

    assert_eq!(loaded.seed, seed);
    assert_eq!(loaded.schedule, recording.schedule);
    assert_eq!(loaded.outcome, outcome);

    // Replay: re-run the *schedule* (never the seed) and confirm the same
    // pathology reproduces.
    let replayed = run(&loaded.schedule, &mut PanicsOnDesync);
    assert_eq!(
        replayed, outcome,
        "replaying the captured schedule must reproduce the exact same outcome"
    );
}

#[test]
fn save_and_load_round_trip_through_a_file() {
    let dir = std::env::temp_dir().join(format!(
        "windows-file-watcher-example-test-harness-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("recording.json");

    let generator = Generator::new();
    let schedule = generator.generate(42);
    let outcome = run(&schedule, &mut PanicsOnDesync);
    let recording = Recording::new(42, schedule, outcome);

    recording.save(&path).expect("save");
    let loaded = Recording::load(&path).expect("load");

    assert_eq!(loaded.seed, recording.seed);
    assert_eq!(loaded.schedule, recording.schedule);
    assert_eq!(loaded.outcome, recording.outcome);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}
