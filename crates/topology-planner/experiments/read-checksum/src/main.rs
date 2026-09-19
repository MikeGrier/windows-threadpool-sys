// Copyright (c) Mike Grier.
#![cfg(windows)]

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

use windows_read_checksum_experiment::{Config, run, write_fixture};

fn execute(args: &[String], output: &mut impl Write) -> io::Result<()> {
    match args {
        [command] if command == "placements" => {
            serde_json::to_writer(
                &mut *output,
                &windows_read_checksum_experiment::discover_placements()?,
            )?;
            writeln!(output)
        }
        [command, path, bytes] if command == "fixture" => {
            let bytes = bytes.parse::<u64>().map_err(io::Error::other)?;
            if bytes == 0 || bytes > 1024 * 1024 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "fixture size must be in 1..=1 GiB",
                ));
            }
            let mut file =
                BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
            write_fixture(&mut file, bytes)?;
            file.flush()?;
            serde_json::to_writer(
                &mut *output,
                &serde_json::json!({"status": "fixture_created", "bytes": bytes}),
            )?;
            writeln!(output)
        }
        [command, config, report] if command == "run" => {
            let config: Config = serde_json::from_reader(File::open(config)?)?;
            let mut report_file = BufWriter::new(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(report)?,
            );
            match run(config) {
                Ok(capture) => {
                    serde_json::to_writer_pretty(&mut report_file, &capture)?;
                    writeln!(report_file)?;
                    report_file.flush()?;
                    serde_json::to_writer(
                        &mut *output,
                        &serde_json::json!({"status": "success", "report": Path::new(report)}),
                    )?;
                    writeln!(output)
                }
                Err(error) => {
                    serde_json::to_writer(
                        &mut report_file,
                        &serde_json::json!({"status": "error", "error": error.to_string()}),
                    )?;
                    report_file.flush()?;
                    Err(error)
                }
            }
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: windows-read-checksum-experiment placements | fixture <new-file> <bytes> | run <config.json> <new-report.json>",
        )),
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mut output = io::stdout().lock();
    match execute(&args, &mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let result = serde_json::to_writer(
                &mut output,
                &serde_json::json!({"status": "error", "error": error.to_string()}),
            )
            .map_err(io::Error::other)
            .and_then(|()| writeln!(output));
            if result.is_err() {
                return ExitCode::from(2);
            }
            ExitCode::FAILURE
        }
    }
}
