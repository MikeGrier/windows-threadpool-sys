// Copyright (c) 2026 Mike Grier
//! Integration mode 3: replay.
//!
//! Loads a [`Recording`] -- captured here in memory rather than from a file, so
//! this example runs standalone -- and re-drives its schedule to reproduce the
//! pathology deterministically. [`src/bin/replay.rs`](../src/bin/replay.rs)
//! does the same thing from a JSON file on disk.
//!
//! ```text
//! cargo run --example replay_demo
//! ```

use windows_file_watcher_example_test_harness::{
    Generator, Recording, example_handler::BuggyHandler, run,
};

fn main() {
    // Stand in for "a recording captured earlier and persisted to disk": find
    // one pathology, serialize it, and only ever touch the JSON from here on.
    let generator = Generator::new();
    let (seed, schedule, outcome) = (0..)
        .find_map(|seed| {
            let schedule = generator.generate(seed);
            let outcome = run(&schedule, &mut BuggyHandler::new());
            outcome
                .pathology()
                .is_some()
                .then_some((seed, schedule, outcome))
        })
        .expect("some seed should trip BuggyHandler's oracle");
    let json = Recording::new(seed, schedule, outcome)
        .to_json()
        .expect("serialize");

    // From here on, this is exactly what `replay` does: load JSON, re-drive the
    // schedule, compare.
    let recording = Recording::from_json(&json).expect("deserialize");
    println!(
        "loaded recording from seed {}, {} step(s)",
        recording.seed,
        recording.schedule.len()
    );

    let replayed = run(&recording.schedule, &mut BuggyHandler::new());
    println!("recorded outcome: {:?}", recording.outcome);
    println!("replayed outcome: {replayed:?}");
    assert_eq!(
        replayed, recording.outcome,
        "replay must reproduce the captured outcome exactly"
    );
    println!("reproduced: identical outcome.");
}
