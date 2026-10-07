// Copyright (c) 2026 Mike Grier
//! `win-job-launcher`: see the library's documentation for what it guarantees.

use std::env;
use std::io;
use std::process::ExitCode;

use win_job_launcher::cli;
use win_job_launcher::launch;
use win_job_launcher::narrate::Narrator;
use win_job_launcher::outcome;

/// The launcher's own exit codes. The command's exit code is in the result
/// file, never here: it is a full 32-bit value and could collide with these.
mod exit_codes {
    /// The command ran to an outcome -- whatever it was -- and the result file
    /// was written.
    pub const REPORTED: u8 = 0;
    /// The command line was refused; nothing ran and no result was written.
    pub const USAGE: u8 = 2;
    /// The command ran, but its result could not be written.
    pub const RESULT_UNWRITTEN: u8 = 3;
}

fn main() -> ExitCode {
    let invocation = match cli::parse(env::args_os().skip(1)) {
        Ok(invocation) => invocation,
        Err(e) => {
            Narrator::new(io::stderr(), false).error(format_args!("{e}\n{}", cli::USAGE));
            return ExitCode::from(exit_codes::USAGE);
        }
    };

    let mut n = Narrator::new(io::stderr(), invocation.trace);
    let report = launch::run(&invocation, &mut n);
    match outcome::write_result(&invocation.result, &report) {
        Ok(()) => {
            n.trace(format_args!("wrote {}", invocation.result.display()));
            ExitCode::from(exit_codes::REPORTED)
        }
        Err(e) => {
            n.error(format_args!(
                "could not write {}: {e}",
                invocation.result.display()
            ));
            ExitCode::from(exit_codes::RESULT_UNWRITTEN)
        }
    }
}
