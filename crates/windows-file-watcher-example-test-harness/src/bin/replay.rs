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

/// Where this bin's diagnostics and result go, kept as one seam (the repo's
/// architectural pre-step, matching
/// `windows-file-watcher/src/bin/run_scenario.rs`) rather than scattering
/// `println!`/`eprintln!` across the file.
///
/// Declared outside the `cfg(windows)` gate so the non-Windows arm reports
/// through the same seam rather than opening a second output site.
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

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    return imp::main();
    #[cfg(not(windows))]
    {
        stdio().diagnostic(
            "windows-file-watcher-example-test-harness is Windows-only; nothing to do here.",
        );
        std::process::ExitCode::FAILURE
    }
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use windows_file_watcher_example_test_harness::{
        Outcome, PathologyKind, Recording, example_handler::BuggyHandler, run_with_deadline,
    };

    use super::{Output, stdio};

    // `replay.rs` is a bin root, so this module's own directory is
    // `src/bin/imp/`; the tests live beside the bin they cover, at
    // `src/bin/replay/tests.rs`.
    #[cfg(test)]
    #[path = "../replay/tests.rs"]
    mod tests;

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

    /// The shortest recorded deadline this bin will honour.
    ///
    /// The upper clamp alone is not enough. A recording is untrusted input, so
    /// it can also name a *too small* deadline -- `deadline_ms: 0` being the
    /// worst case -- which expires before the handler has run at all. Since a
    /// `Stalled` replay is compared semantically rather than on its deadline
    /// value, that would report a wedge as reproduced without ever giving the
    /// handler a chance to wedge, turning this bin into something that confirms
    /// whatever it is handed. Clamping up to a floor makes an unreproducible
    /// recording fail honestly instead.
    const MIN_REPLAY_DEADLINE: Duration = Duration::from_millis(100);
    pub fn main() -> std::process::ExitCode {
        use std::process::ExitCode;

        let mut output = stdio();
        let Some(path) = std::env::args().nth(1) else {
            output.diagnostic("usage: replay <path-to-recording.json>");
            return ExitCode::FAILURE;
        };
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
        // A recording is untrusted input -- that is why the deadline is clamped
        // at both ends below -- so a malformed or missing one is an ordinary
        // failure to report, not a panic.
        let recording = match Recording::load(path) {
            Ok(recording) => recording,
            Err(error) => {
                output.diagnostic(&format!(
                    "error: could not load '{path}' as a recording: {error}"
                ));
                return false;
            }
        };

        output.report(&format!(
            "loaded {path}: seed {}, {} step(s)",
            recording.seed,
            recording.schedule.len()
        ));
        output.report(&format!("recorded outcome: {:?}", recording.outcome));

        let deadline = replay_deadline(&recording.outcome, output);
        let replayed = run_with_deadline(&recording.schedule, BuggyHandler::new(), deadline);
        output.report(&format!("replayed outcome: {replayed:?}"));

        match reproduces(&recording.outcome, &replayed) {
            Reproduction::Confirmed => {
                output.report("reproduced: identical outcome.");
                true
            }
            Reproduction::Unverifiable => {
                // Not a divergence and not a reproduction: the replay waited
                // less long than the recording did, so it cannot establish the
                // recorded stall either way.
                output.diagnostic(
                    "UNVERIFIABLE: the recorded deadline was clamped down, so this replay only \
                     shows the handler exceeded the shorter deadline -- not that it would have \
                     exceeded the recorded one. Re-record with a deadline within this bin's cap.",
                );
                false
            }
            Reproduction::Diverged => {
                // A schedule-caused pathology (this bin's whole point) should
                // always reproduce; divergence here would mean the harness's own
                // determinism promise was broken, not a fidelity-limit case (that
                // limit concerns a *handler's own* nondeterminism, not the harness).
                output.diagnostic("NOT reproduced: outcome differs from the recording.");
                false
            }
        }
    }

    /// The deadline to replay `recorded` under.
    ///
    /// A recording is untrusted input, so its `deadline_ms` is clamped into
    /// `MIN_REPLAY_DEADLINE..=MAX_REPLAY_DEADLINE` at both ends: too large and
    /// it defeats replay's cannot-hang guarantee, too small and it expires
    /// before the handler has run, which -- since `reproduces` compares
    /// `Stalled` semantically -- would report a wedge that never happened.
    fn replay_deadline(
        recorded: &Outcome,
        output: &mut Output<impl std::io::Write, impl std::io::Write>,
    ) -> Duration {
        let Outcome::Pathology(PathologyKind::Stalled { deadline_ms }) = recorded else {
            return DEFAULT_REPLAY_DEADLINE;
        };
        let requested = Duration::from_millis(u64::try_from(*deadline_ms).unwrap_or(u64::MAX));
        let clamped = requested.clamp(MIN_REPLAY_DEADLINE, MAX_REPLAY_DEADLINE);
        if clamped != requested {
            output.diagnostic(&format!(
                "recorded deadline {requested:?} is outside this bin's \
                 {MIN_REPLAY_DEADLINE:?}..={MAX_REPLAY_DEADLINE:?} range; clamping to {clamped:?} \
                 so an untrusted recording can neither hang the replay nor expire before the \
                 handler runs"
            ));
        }
        clamped
    }

    /// What a replay established about the recorded outcome.
    #[derive(Debug, PartialEq, Eq)]
    enum Reproduction {
        /// The replay confirms the recorded outcome.
        Confirmed,
        /// The replay is consistent with the recording but does not establish
        /// it -- see [`reproduces`].
        Unverifiable,
        /// The replay produced a different outcome.
        Diverged,
    }

    /// What `replayed` establishes about `recorded`.
    ///
    /// Exact equality everywhere except `Stalled`, where `deadline_ms` is the
    /// run's *configuration* rather than anything the handler did: comparing it
    /// exactly would report a clamped stall as a divergence even though the
    /// handler wedged exactly as recorded.
    ///
    /// The comparison is **one-directional**, which is the subtle part. A stall
    /// only proves the handler exceeded the deadline it ran under, so a replay
    /// establishes the recording *only when it waited at least as long*. Replay
    /// under a shorter deadline -- which the `MAX_REPLAY_DEADLINE` clamp
    /// produces for an oversized recording -- proves the handler exceeded the
    /// shorter one, and says nothing about whether it would have finished before
    /// the recorded one. Calling that a reproduction would let a recording
    /// claiming a 600s stall be "confirmed" by a handler that merely took 300s.
    fn reproduces(recorded: &Outcome, replayed: &Outcome) -> Reproduction {
        match (recorded, replayed) {
            (
                Outcome::Pathology(PathologyKind::Stalled {
                    deadline_ms: recorded_ms,
                }),
                Outcome::Pathology(PathologyKind::Stalled {
                    deadline_ms: replayed_ms,
                }),
            ) => {
                if replayed_ms >= recorded_ms {
                    Reproduction::Confirmed
                } else {
                    Reproduction::Unverifiable
                }
            }
            _ if recorded == replayed => Reproduction::Confirmed,
            _ => Reproduction::Diverged,
        }
    }
}
