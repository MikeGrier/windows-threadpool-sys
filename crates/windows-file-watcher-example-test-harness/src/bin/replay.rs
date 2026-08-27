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
    use std::io::{self, Write};

    use windows_file_watcher_example_test_harness::{
        Recording, example_handler::BuggyHandler, run,
    };

    pub fn main() {
        let path = std::env::args()
            .nth(1)
            .expect("usage: replay <path-to-recording.json>");
        // All reporting -- including the non-reproduction case -- is routed
        // through one writer (repository architecture rule: never call
        // print!/eprintln! from more than one site), so the destination stays
        // separable from the formatting at each call site.
        let reproduced = replay(&path, &mut io::stdout().lock());
        if !reproduced {
            std::process::exit(1);
        }
    }

    /// Replay the recording at `path` against the example handler, reporting
    /// through `out`. Returns whether the recorded pathology reproduced.
    fn replay(path: &str, out: &mut impl Write) -> bool {
        let recording = Recording::load(path).expect("load recording");

        writeln!(
            out,
            "loaded {path}: seed {}, {} step(s)",
            recording.seed,
            recording.schedule.len()
        )
        .expect("write");
        writeln!(out, "recorded outcome: {:?}", recording.outcome).expect("write");

        let mut handler = BuggyHandler::new();
        let replayed = run(&recording.schedule, &mut handler);
        writeln!(out, "replayed outcome: {replayed:?}").expect("write");

        if replayed == recording.outcome {
            writeln!(out, "reproduced: identical outcome.").expect("write");
            true
        } else {
            // A schedule-caused pathology (this bin's whole point) should always
            // reproduce; divergence here would mean the harness's own
            // determinism promise was broken, not a fidelity-limit case (that
            // limit concerns a *handler's own* nondeterminism, not the harness).
            writeln!(out, "NOT reproduced: outcome differs from the recording.").expect("write");
            false
        }
    }
}
