// Copyright (c) Mike Grier.
#![cfg(windows)]

mod ingest_capture;
mod keyed_capture;

use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use serde::Serialize;
use windows_request_reply_experiment::{
    Arrangement, Config, Report, ResponseSummary, response_summary, run, trace,
};

#[derive(Serialize)]
struct Trial {
    scenario: &'static str,
    position: usize,
    aggregate: ResponseSummary,
    lanes: Vec<ResponseSummary>,
    report: Report,
}

fn capture() -> io::Result<Vec<Trial>> {
    let mut trials = Vec::new();
    for (scenario, count, burst, interval, skew, capacity, cancel_after) in [
        ("empty", 0, 1, 0, false, 8, None),
        ("steady", 128, 1, 100_000, false, 8, None),
        ("burst", 128, 16, 1_000_000, false, 8, None),
        ("skewed", 128, 32, 1_000_000, true, 8, None),
        ("pressure", 128, 128, 0, true, 2, None),
        ("cancellation", 128, 128, 0, true, 8, Some(32)),
    ] {
        let requests = trace(count, 2, burst, interval, skew)?;
        let config = Config {
            workers: 2,
            capacity,
            timeout_ms: 5000,
            cancel_after_admitted: cancel_after,
        };
        for (position, arrangement) in [
            Arrangement::Shared,
            Arrangement::Assigned,
            Arrangement::Assigned,
            Arrangement::Shared,
        ]
        .into_iter()
        .enumerate()
        {
            let report = run(
                config.clone(),
                requests.clone(),
                arrangement,
                &AtomicBool::new(false),
            )?;
            let aggregate = response_summary(&report, None)?;
            let lanes = (0..config.workers)
                .map(|lane| response_summary(&report, Some(lane)))
                .collect::<io::Result<_>>()?;
            trials.push(Trial {
                scenario,
                position,
                aggregate,
                lanes,
                report,
            });
        }
    }
    Ok(trials)
}

fn execute(args: &[String], output: &mut impl Write) -> io::Result<()> {
    let [command, path] = args else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: windows-request-reply-experiment capture|capture-stateful|capture-ingest <new-report.json>",
        ));
    };
    if !matches!(
        command.as_str(),
        "capture" | "capture-stateful" | "capture-ingest"
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown command",
        ));
    }
    let mut report = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    let (schema, result) = match command.as_str() {
        "capture-stateful" => ("stateful-capture-v1", keyed_capture::capture()),
        "capture-ingest" => ("ingest-capture-v1", ingest_capture::capture()),
        _ => (
            "request-reply-capture-v1",
            capture().and_then(|trials| serde_json::to_value(trials).map_err(io::Error::other)),
        ),
    };
    match result {
        Ok(trials) => {
            let record = serde_json::json!({"schema": schema, "status": "success",
                "evidence_class": "offline_unpinned_behavioral_experiment", "debug_assertions": cfg!(debug_assertions),
                "recorded_unix_seconds": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(io::Error::other)?.as_secs(),
                "build": {"git_revision": env!("RR_REVISION"), "worktree": env!("RR_WORKTREE"),
                    "source_and_manifests_fnv64": env!("RR_SOURCE_FNV64"), "compiler": env!("RR_COMPILER"),
                    "target": env!("RR_TARGET"), "opt_level": env!("RR_OPT_LEVEL"), "rustflags": env!("RR_RUSTFLAGS")},
                "trials": trials});
            serde_json::to_writer_pretty(&mut report, &record)?;
            writeln!(report)?;
            report.flush()?;
            serde_json::to_writer(
                &mut *output,
                &serde_json::json!({"status": "success", "report": Path::new(path)}),
            )?;
            writeln!(output)
        }
        Err(error) => {
            serde_json::to_writer(
                &mut report,
                &serde_json::json!({"status": "error", "error": error.to_string()}),
            )?;
            report.flush()?;
            Err(error)
        }
    }
}

fn main() -> ExitCode {
    let mut output = io::stdout().lock();
    match execute(&std::env::args().skip(1).collect::<Vec<_>>(), &mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let written = serde_json::to_writer(
                &mut output,
                &serde_json::json!({"status": "error", "error": error.to_string()}),
            )
            .map_err(io::Error::other)
            .and_then(|()| writeln!(output));
            if written.is_ok() {
                ExitCode::FAILURE
            } else {
                ExitCode::from(2)
            }
        }
    }
}
