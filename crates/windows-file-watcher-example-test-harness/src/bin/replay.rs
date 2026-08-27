// Copyright (c) 2026 Mike Grier
//! `replay`: load a captured JSON
//! [`Recording`](windows_file_watcher_example_test_harness::Recording) and
//! re-drive its schedule against the same built-in example handler, reporting
//! whether the pathology reproduces.
//!
//! Handler-linked, like `capture` (crate DESIGN-NOTES D-3): swap in your own
//! `Handler` to replay a recording against your own code.
//!
//! ```text
//! cargo run --bin replay -- <path-to-recording.json>
//! ```

fn main() {
    #[cfg(windows)]
    imp::main();
    #[cfg(not(windows))]
    eprintln!("windows-file-watcher-example-test-harness is Windows-only; nothing to do here.");
}

#[cfg(windows)]
mod imp {
    use windows_file_watcher_example_test_harness::{
        Recording, example_handler::BuggyHandler, run,
    };

    pub fn main() {
        let path = std::env::args()
            .nth(1)
            .expect("usage: replay <path-to-recording.json>");
        let recording = Recording::load(&path).expect("load recording");

        println!(
            "loaded {path}: seed {}, {} step(s)",
            recording.seed,
            recording.schedule.len()
        );
        println!("recorded outcome: {:?}", recording.outcome);

        let mut handler = BuggyHandler::new();
        let replayed = run(&recording.schedule, &mut handler);
        println!("replayed outcome: {replayed:?}");

        if replayed == recording.outcome {
            println!("reproduced: identical outcome.");
        } else {
            // A schedule-caused pathology (this bin's whole point) should always
            // reproduce; divergence here would mean the harness's own
            // determinism promise was broken, not a fidelity-limit case (that
            // limit concerns a *handler's own* nondeterminism, not the harness).
            eprintln!("NOT reproduced: outcome differs from the recording.");
            std::process::exit(1);
        }
    }
}
