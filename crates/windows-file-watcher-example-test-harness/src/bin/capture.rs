// Copyright (c) 2026 Mike Grier
//! `capture`: run the seeded generator across a range of seeds against a
//! built-in example handler, and save every schedule that trips an oracle as a
//! JSON [`Recording`](windows_file_watcher_example_test_harness::Recording).
//!
//! This bin is a **handler-linked exemplar** (crate DESIGN-NOTES D-3): it always
//! drives [`example_handler::BuggyHandler`], a small, intentionally-buggy
//! handler shipped so this bin has something to find. Write your own capture
//! bin against your own `Handler` the same way: generate a schedule, `run` it,
//! and `save` whatever trips your handler's oracle.
//!
//! ```text
//! cargo run --bin capture -- [--seeds N] [--start N] [--out DIR]
//! ```
//!
//! [`example_handler`]: windows_file_watcher_example_test_harness::example_handler
//! [`example_handler::BuggyHandler`]: windows_file_watcher_example_test_harness::example_handler::BuggyHandler

/// Where this bin's diagnostics and result lines go, kept as one seam (the
/// repo's architectural pre-step, matching
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
    /// A usage or error line, to stderr.
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
    use std::path::PathBuf;

    use windows_file_watcher_example_test_harness::{
        Generator, Recording, example_handler::BuggyHandler, run,
    };

    use super::{Output, stdio};

    pub fn main() -> std::process::ExitCode {
        let mut output = stdio();
        let Some(args) = Args::parse(&mut output) else {
            return std::process::ExitCode::FAILURE;
        };
        capture(&args, &mut output);
        std::process::ExitCode::SUCCESS
    }

    fn capture(args: &Args, output: &mut Output<impl std::io::Write, impl std::io::Write>) {
        std::fs::create_dir_all(&args.out).expect("create output directory");
        // Cannot overflow: `Args::parse_from` rejects a range that would, so
        // the check lives in one place rather than being restated here.
        let end = args.start + args.seeds;

        let generator = Generator::new();
        let mut found = 0usize;
        for seed in args.start..end {
            let schedule = generator.generate(seed);
            let mut handler = BuggyHandler::new();
            let outcome = run(&schedule, &mut handler);
            if let Some(pathology) = outcome.pathology() {
                let recording = Recording::new(seed, schedule, outcome.clone());
                let path = args.out.join(format!("capture-{seed}.json"));
                recording.save(&path).expect("save recording");
                output.report(&format!("seed {seed}: {pathology:?} -> {}", path.display()));
                found += 1;
            }
        }
        output.report(&format!(
            "checked {} seed(s) [{}, {end}), captured {found} pathology(ies) into {}",
            args.seeds,
            args.start,
            args.out.display(),
        ));
    }

    /// Minimal hand-rolled argument parsing -- deliberately no CLI-argument
    /// dependency, so this exemplar stays as small as its purpose warrants.
    struct Args {
        seeds: u64,
        start: u64,
        out: PathBuf,
    }

    impl Args {
        /// Parse, or report the usage error and give back `None`.
        ///
        /// A malformed command line is an ordinary usage error, not a panic:
        /// it goes through the same output seam as everything else this bin
        /// prints, matching `replay` and `windows-file-watcher`'s
        /// `run_scenario`.
        fn parse(output: &mut Output<impl std::io::Write, impl std::io::Write>) -> Option<Self> {
            Self::parse_from(std::env::args().skip(1), output)
        }

        /// The parser proper, over any argument sequence.
        ///
        /// Separated from [`Args::parse`] so the error branches are reachable
        /// from tests without a process; `parse` supplies the real command line.
        fn parse_from(
            args: impl IntoIterator<Item = String>,
            output: &mut Output<impl std::io::Write, impl std::io::Write>,
        ) -> Option<Self> {
            const USAGE: &str = "usage: capture [--seeds N] [--start N] [--out DIR]";

            let mut seeds = 1000u64;
            let mut start = 0u64;
            let mut out = PathBuf::from("captures");
            let mut args = args.into_iter();
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--seeds" | "--start" => {
                        let Some(raw) = args.next() else {
                            output.diagnostic(&format!("error: {flag} needs a value\n{USAGE}"));
                            return None;
                        };
                        let Ok(value) = raw.parse::<u64>() else {
                            output.diagnostic(&format!(
                                "error: {flag} must be a number, got '{raw}'\n{USAGE}"
                            ));
                            return None;
                        };
                        if flag == "--seeds" {
                            seeds = value;
                        } else {
                            start = value;
                        }
                    }
                    "--out" => {
                        let Some(raw) = args.next() else {
                            output.diagnostic(&format!("error: --out needs a value\n{USAGE}"));
                            return None;
                        };
                        out = PathBuf::from(raw);
                    }
                    other => {
                        output.diagnostic(&format!(
                            "error: unrecognized argument '{other}'\n{USAGE}"
                        ));
                        return None;
                    }
                }
            }
            if seeds == 0 {
                output.diagnostic(&format!(
                    "error: --seeds must be at least 1; nothing would be checked\n{USAGE}"
                ));
                return None;
            }
            if start.checked_add(seeds).is_none() {
                output.diagnostic(&format!(
                    "error: --start {start} + --seeds {seeds} overflows a u64\n{USAGE}"
                ));
                return None;
            }
            Some(Self { seeds, start, out })
        }
    }

    // `capture.rs` is a bin root, so this module's own directory is
    // `src/bin/imp/`; the tests live beside the bin they cover.
    #[cfg(test)]
    #[path = "../capture/tests.rs"]
    mod tests;
}
