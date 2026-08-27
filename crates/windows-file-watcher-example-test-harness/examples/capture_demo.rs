// Copyright (c) 2026 Mike Grier
//! Integration mode 2: capture.
//!
//! A miniature version of the `capture` bin, inlined here so the technique
//! reads top to bottom in one file: run the seeded generator across a range of
//! seeds against a handler, and keep whatever trips its oracle -- in memory,
//! here, rather than to disk. Write your own capture loop against your own
//! `Handler` exactly this way; see
//! [`src/bin/capture.rs`](../src/bin/capture.rs) for the fuller CLI tool with
//! argument parsing and on-disk output.
//!
//! ```text
//! cargo run --example capture_demo
//! ```

use std::io::{self, Write};

use windows_file_watcher_example_test_harness::{
    Generator, Recording, example_handler::BuggyHandler, run,
};

fn main() {
    // All reporting is routed through one writer (repository architecture
    // rule: never call print!/eprintln! from more than one site).
    let mut out = io::stdout().lock();
    let generator = Generator::new();
    let mut recordings = Vec::new();

    for seed in 0..10 {
        let schedule = generator.generate(seed);
        let mut handler = BuggyHandler::new();
        let outcome = run(&schedule, &mut handler);
        if let Some(pathology) = outcome.pathology() {
            writeln!(out, "seed {seed}: {pathology:?}").expect("write");
            recordings.push(Recording::new(seed, schedule, outcome));
        }
    }

    writeln!(
        out,
        "checked 10 seed(s), captured {} pathology(ies) in memory",
        recordings.len()
    )
    .expect("write");
    assert!(
        !recordings.is_empty(),
        "the default generator config should find at least one pathology in 10 seeds"
    );

    // Any of these could be persisted with `Recording::save` for later replay,
    // exactly as `src/bin/capture.rs` does.
    let first = &recordings[0];
    writeln!(
        out,
        "first captured recording, as JSON:\n{}",
        first.to_json().expect("serialize")
    )
    .expect("write");
}
