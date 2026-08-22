// Copyright (c) 2026 Mike Grier
//! `run-scenario`: replays a persisted JSON scenario file through the
//! same model/harness the `scenario_stress` integration test uses (M9.5).
//! See [`windows_file_watcher::scenario`] for why the JSON schema this reads
//! is not part of the crate's semver contract.

use std::process::ExitCode;

use windows_file_watcher::scenario::{HarnessParams, run_scenario, seed};

fn main() -> ExitCode {
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

    let scenario = match serde_json::from_str(&contents) {
        Ok(scenario) => scenario,
        Err(error) => {
            eprintln!("error: '{path}' is not a valid scenario: {error}");
            return ExitCode::FAILURE;
        }
    };

    let params = HarnessParams::for_operation_count(
        windows_file_watcher::scenario::Scenario::operation_count(&scenario),
    );
    let outcome = run_scenario(&scenario, seed(), &params);
    println!("{outcome:#?}");
    ExitCode::SUCCESS
}
