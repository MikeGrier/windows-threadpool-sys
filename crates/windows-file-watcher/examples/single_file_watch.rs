// Copyright (c) 2026 Mike Grier
//! Watch a single file (D-7): the parent directory is watched non-recursively
//! and filtered to the file's own leaf name.
//!
//! ```text
//! cargo run --example single_file_watch -- C:\some\directory\file.txt
//! ```

use std::env;
use std::process::ExitCode;

use windows_file_watcher::{Monitor, Notification, WatchOptions};

/// Where this example's diagnostics and change reports go, kept as one seam
/// (the repo's architectural pre-step) rather than scattering
/// `eprintln!`/`println!` across the file.
struct Output<E, O> {
    stderr: E,
    stdout: O,
}

impl<E: std::io::Write, O: std::io::Write> Output<E, O> {
    fn diagnostic(&mut self, message: &str) {
        let _ = writeln!(self.stderr, "{message}");
    }

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

fn main() -> ExitCode {
    let mut output = stdio();
    let Some(path) = env::args().nth(1) else {
        output.diagnostic("usage: single_file_watch <file>");
        return ExitCode::FAILURE;
    };

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(&path, WatchOptions::new())
        .expect("register the subscription");

    output.report(&format!("watching {path}; press Ctrl+C to stop"));

    while let Some(notification) = receiver.recv() {
        match notification {
            Notification::Batch { changes, .. } => {
                for change in changes {
                    output.report(&format!("{:?}", change.kind));
                }
            }
            Notification::Desync { cause, .. } => {
                output.report(&format!(
                    "desync ({cause:?}): re-check the file's current state"
                ));
            }
            Notification::Completion { outcome, .. } => {
                output.report(&format!("registration: {outcome:?}"));
            }
            _ => {}
        }
    }

    drop(watch);
    ExitCode::SUCCESS
}
