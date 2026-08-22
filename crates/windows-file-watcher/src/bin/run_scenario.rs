// Copyright (c) 2026 Mike Grier
//! `run-scenario`: replays a persisted JSON scenario file through the
//! same model/harness the `scenario_stress` integration test uses (M9.5).
//! See [`windows_file_watcher::scenario`] for why the JSON schema this reads
//! is not part of the crate's semver contract.
//!
//! `windows_file_watcher::scenario` only exists on Windows (the crate itself
//! is Windows-only, D-1), so this binary is a no-op stub everywhere else --
//! matching the library's own "resolves to an empty crate" convention --
//! rather than a `[[bin]]` target that fails to build off Windows.

/// Where this tool's diagnostics and result go, kept as one seam (the repo's
/// architectural pre-step) rather than scattering `eprintln!`/`println!`
/// across the binary: destination and formatting stay separable from the
/// call sites that produce content.
struct Output<E, O> {
    stderr: E,
    stdout: O,
}

impl<E: std::io::Write, O: std::io::Write> Output<E, O> {
    /// A usage/error line, to stderr.
    fn diagnostic(&mut self, message: &str) {
        let _ = writeln!(self.stderr, "{message}");
    }

    /// The scenario's outcome, to stdout.
    fn result(&mut self, message: &str) {
        let _ = writeln!(self.stdout, "{message}");
    }
}

fn stdio() -> Output<std::io::Stderr, std::io::Stdout> {
    Output {
        stderr: std::io::stderr(),
        stdout: std::io::stdout(),
    }
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::process::ExitCode;

    use windows_file_watcher::scenario::{HarnessParams, Scenario, run_scenario, seed};

    let mut output = stdio();
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        output.diagnostic("usage: run-scenario <scenario.json>");
        return ExitCode::FAILURE;
    };

    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => {
            output.diagnostic(&format!("error: could not read '{path}': {error}"));
            return ExitCode::FAILURE;
        }
    };

    let scenario: Scenario = match serde_json::from_str(&contents) {
        Ok(scenario) => scenario,
        Err(error) => {
            output.diagnostic(&format!("error: '{path}' is not a valid scenario: {error}"));
            return ExitCode::FAILURE;
        }
    };

    let params = HarnessParams::for_operation_count(scenario.operation_count());
    let outcome = run_scenario(&scenario, seed(), &params);
    output.result(&format!("{outcome:#?}"));
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    stdio().diagnostic("run-scenario is only available on Windows");
    std::process::ExitCode::FAILURE
}
