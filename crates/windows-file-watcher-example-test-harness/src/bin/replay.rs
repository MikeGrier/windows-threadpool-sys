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

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    return imp::main();
    #[cfg(not(windows))]
    {
        eprintln!("windows-file-watcher-example-test-harness is Windows-only; nothing to do here.");
        std::process::ExitCode::FAILURE
    }
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use windows_file_watcher_example_test_harness::{
        Outcome, PathologyKind, Recording, example_handler::BuggyHandler, run_with_deadline,
    };

    /// Bound applied when the recording's own outcome does not name a
    /// [`PathologyKind::Stalled`] deadline. Generous, not tight: this only
    /// needs to catch a genuine wedge, not race a legitimately-slow handler.
    const DEFAULT_REPLAY_DEADLINE: Duration = Duration::from_secs(30);

    /// Hard ceiling applied to a *recorded* [`PathologyKind::Stalled`]
    /// deadline. `deadline_ms` comes from an externally loaded JSON
    /// recording with no upper bound of its own -- an oversized or malicious
    /// value (or `u64::try_from`'s own `u64::MAX` fallback for a value that
    /// does not fit) would otherwise make this bin wait for it verbatim,
    /// defeating replay's whole "cannot hang" guarantee. Generous enough that
    /// a legitimate wedge test's deadline is never actually reached by this
    /// clamp in practice.
    const MAX_REPLAY_DEADLINE: Duration = Duration::from_secs(300);

    /// Where this bin's diagnostics and result go, kept as one seam (the
    /// repo's architectural pre-step, matching
    /// `windows-file-watcher/src/bin/run_scenario.rs`) rather than scattering
    /// `println!`/`eprintln!` across the file.
    struct Output<E, O> {
        stderr: E,
        stdout: O,
    }

    impl<E: std::io::Write, O: std::io::Write> Output<E, O> {
        /// A usage/error line, to stderr.
        fn diagnostic(&mut self, message: &str) {
            let _ = writeln!(self.stderr, "{message}");
        }

        /// A progress or result line, to stdout.
        fn report(&mut self, message: &str) {
            let _ = writeln!(self.stdout, "{message}");
        }
    }

    fn stdio() -> Output<std::io::Stderr, std::io::Stdout> {
        Output {
            stderr: std::io::stderr(),
            stdout: std::io::stdout(),
        }
    }

    pub fn main() -> std::process::ExitCode {
        use std::process::ExitCode;

        let path = std::env::args()
            .nth(1)
            .expect("usage: replay <path-to-recording.json>");
        let mut output = stdio();
        if replay(&path, &mut output) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }

    /// Replay the recording at `path` against the example handler, reporting
    /// through `output`. Returns whether the recorded pathology reproduced.
    ///
    /// Always replays through [`run_with_deadline`], never a plain `run`: a
    /// recording's `outcome` is arbitrary (it may have come from any capture
    /// tool, not only this crate's own `capture` bin), so a recorded
    /// `Stalled` must be replayed under that same deadline rather than
    /// hanging this process forever, and a recorded `HarnessPanicked` must be
    /// replayed under `catch_unwind` rather than letting that panic escape
    /// and crash the replay itself.
    fn replay(path: &str, output: &mut Output<impl std::io::Write, impl std::io::Write>) -> bool {
        let recording = Recording::load(path).expect("load recording");

        output.report(&format!(
            "loaded {path}: seed {}, {} step(s)",
            recording.seed,
            recording.schedule.len()
        ));
        output.report(&format!("recorded outcome: {:?}", recording.outcome));

        let deadline = match &recording.outcome {
            Outcome::Pathology(PathologyKind::Stalled { deadline_ms }) => {
                let requested =
                    Duration::from_millis(u64::try_from(*deadline_ms).unwrap_or(u64::MAX));
                let clamped = requested.min(MAX_REPLAY_DEADLINE);
                if clamped != requested {
                    output.diagnostic(&format!(
                        "recorded deadline {requested:?} exceeds this bin's {MAX_REPLAY_DEADLINE:?} \
                         cap; clamping so replay cannot be made to hang by an untrusted recording"
                    ));
                }
                clamped
            }
            _ => DEFAULT_REPLAY_DEADLINE,
        };
        let replayed = run_with_deadline(&recording.schedule, BuggyHandler::new(), deadline);
        output.report(&format!("replayed outcome: {replayed:?}"));

        if reproduces(&recording.outcome, &replayed) {
            output.report("reproduced: identical outcome.");
            true
        } else {
            // A schedule-caused pathology (this bin's whole point) should always
            // reproduce; divergence here would mean the harness's own
            // determinism promise was broken, not a fidelity-limit case (that
            // limit concerns a *handler's own* nondeterminism, not the harness).
            output.diagnostic("NOT reproduced: outcome differs from the recording.");
            false
        }
    }

    /// Whether `replayed` reproduces `recorded`.
    ///
    /// Equality everywhere except `Stalled`, where the `deadline_ms` is the
    /// run's *configuration* rather than anything the handler did. Clamping an
    /// oversized recorded deadline (so an untrusted recording cannot hang this
    /// process) necessarily changes that field, and comparing it exactly would
    /// report every clamped stall as "NOT reproduced" even though the handler
    /// wedged exactly as recorded. What reproduces is the wedge; the deadline is
    /// only how long we waited before calling it one.
    fn reproduces(recorded: &Outcome, replayed: &Outcome) -> bool {
        match (recorded, replayed) {
            (
                Outcome::Pathology(PathologyKind::Stalled { .. }),
                Outcome::Pathology(PathologyKind::Stalled { .. }),
            ) => true,
            _ => recorded == replayed,
        }
    }
}
