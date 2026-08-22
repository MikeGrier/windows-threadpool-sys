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

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::process::ExitCode;

    use windows_file_watcher::scenario::{HarnessParams, Scenario, run_scenario, seed};

    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: run-scenario <scenario.json>");
        return ExitCode::FAILURE;
    };

    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!("error: could not read '{path}': {error}");
            return ExitCode::FAILURE;
        }
    };

    let scenario: Scenario = match serde_json::from_str(&contents) {
        Ok(scenario) => scenario,
        Err(error) => {
            eprintln!("error: '{path}' is not a valid scenario: {error}");
            return ExitCode::FAILURE;
        }
    };

    let params = HarnessParams::for_operation_count(scenario.operation_count());
    let outcome = run_scenario(&scenario, seed(), &params);
    println!("{outcome:#?}");
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("run-scenario is only available on Windows");
    std::process::ExitCode::FAILURE
}
