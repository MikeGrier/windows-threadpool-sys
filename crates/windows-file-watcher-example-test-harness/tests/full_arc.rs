// Copyright (c) 2026 Mike Grier
//! M6.3: the full arc, in one integration test -- generate, run, record,
//! replay -- tying every milestone together end to end, the way a downstream
//! consumer would read the whole story in one place.

#![cfg(windows)]

use windows_file_watcher_example_test_harness::{
    Generator, GeneratorConfig, Recording, example_handler::BuggyHandler, run,
};

#[test]
fn the_full_arc_generate_run_record_replay() {
    // 1. Generate a contract-legal schedule from a seed (M2).
    let generator = Generator::with_config(GeneratorConfig {
        watches: 2,
        steps_per_watch: 12,
        ..GeneratorConfig::default()
    });
    let seed = 7;
    let schedule = generator.generate(seed);

    // 2. Run it against a handler, catching any pathology (M3).
    let outcome = run(&schedule, &mut BuggyHandler::new());
    let pathology = outcome
        .pathology()
        .cloned()
        .expect("seed 7 with this config should trip BuggyHandler's oracle");

    // 3. Record it as a portable JSON artifact (M4).
    let recording = Recording::new(seed, schedule, outcome.clone());
    let json = recording.to_json().expect("serialize");

    // 4. Load the artifact back -- simulating "a teammate downloaded this CI
    //    failure artifact" -- and replay it (M4/M5's technique).
    let loaded = Recording::from_json(&json).expect("deserialize");
    let replayed = run(&loaded.schedule, &mut BuggyHandler::new());

    // The whole point of the arc: the replayed outcome is identical,
    // deterministically, with nothing but the JSON carried across the gap.
    assert_eq!(replayed, outcome);
    assert_eq!(replayed.pathology(), Some(&pathology));
}
