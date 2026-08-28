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

use windows_file_watcher_example_test_harness::{
    Generator, Recording, example_handler::BuggyHandler, run,
};

/// Where this example's reports go, kept as one seam (the repo's
/// architectural pre-step, matching
/// `windows-file-watcher/examples/minimal_directory_watch.rs`) rather than
/// scattering `println!` across the file.
struct Output<O> {
    stdout: O,
}

impl<O: std::io::Write> Output<O> {
    fn report(&mut self, message: &str) {
        let _ = writeln!(self.stdout, "{message}");
    }
}

fn stdio() -> Output<std::io::Stdout> {
    Output {
        stdout: std::io::stdout(),
    }
}

fn main() {
    let mut output = stdio();
    let generator = Generator::new();
    let mut recordings = Vec::new();

    for seed in 0..10 {
        let schedule = generator.generate(seed);
        let mut handler = BuggyHandler::new();
        let outcome = run(&schedule, &mut handler);
        if let Some(pathology) = outcome.pathology() {
            output.report(&format!("seed {seed}: {pathology:?}"));
            recordings.push(Recording::new(seed, schedule, outcome));
        }
    }

    output.report(&format!(
        "checked 10 seed(s), captured {} pathology(ies) in memory",
        recordings.len()
    ));
    assert!(
        !recordings.is_empty(),
        "the default generator config should find at least one pathology in 10 seeds"
    );

    // Any of these could be persisted with `Recording::save` for later replay,
    // exactly as `src/bin/capture.rs` does.
    let first = &recordings[0];
    output.report(&format!(
        "first captured recording, as JSON:\n{}",
        first.to_json().expect("serialize")
    ));
}
