// Copyright (c) 2026 Mike Grier
//! Watch a directory recursively and print every change.
//!
//! ```text
//! cargo run --example minimal_directory_watch -- C:\some\directory
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
        output.diagnostic("usage: minimal_directory_watch <directory>");
        return ExitCode::FAILURE;
    };

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(&path, WatchOptions::new().subtree(true))
        .expect("register the subscription");

    output.report(&format!(
        "watching {path} (recursively); press Ctrl+C to stop"
    ));

    while let Some(notification) = receiver.recv() {
        match notification {
            Notification::Batch { changes, .. } => {
                for change in changes {
                    output.report(&format!(
                        "{:?} {}",
                        change.kind,
                        change.name.to_path_buf().display()
                    ));
                }
            }
            Notification::Desync { cause, .. } => {
                output.report(&format!("desync ({cause:?}): re-scan the directory"));
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
