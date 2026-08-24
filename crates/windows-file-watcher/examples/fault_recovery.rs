// Copyright (c) 2026 Mike Grier
//! Watch a directory with the interactive retry protocol (D-27): on a fault the
//! monitor asks how long to wait, and this example always answers with the
//! same fixed delay, demonstrating the exchange without any real policy logic.
//!
//! Delete and recreate the watched directory while this is running to see the
//! `Suspended` -> `RetryQuestion` -> `Resumed`/`Established` sequence.
//!
//! ```text
//! cargo run --example fault_recovery -- C:\some\directory
//! ```

use std::env;
use std::process::ExitCode;
use std::time::Duration;

use windows_file_watcher::{Monitor, Notification, RetryMode, VolumeChangeDecision, WatchOptions};

/// The delay this example always answers with, regardless of which operation
/// faulted or how many times. A real client might inspect `operation` and
/// track how many consecutive faults this subscription has seen instead.
const OUR_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Where this example's diagnostics and event reports go, kept as one seam
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
        output.diagnostic("usage: fault_recovery <directory>");
        return ExitCode::FAILURE;
    };

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(
            &path,
            WatchOptions::new()
                .retry(RetryMode::Interactive)
                .report_liveness(true),
        )
        .expect("register the subscription");

    output.report(&format!("watching {path}; press Ctrl+C to stop"));

    while let Some(notification) = receiver.recv() {
        match notification {
            Notification::Batch { changes, .. } => {
                output.report(&format!("{} change(s)", changes.len()));
            }
            Notification::Desync { cause, .. } => {
                output.report(&format!("desync ({cause:?}): re-scan the directory"));
            }
            Notification::Suspended { .. } => {
                output.report("suspended: the monitor is working to re-establish the watch");
            }
            Notification::Resumed { .. } => {
                output.report("resumed: delivering again");
            }
            Notification::Established { mode, .. } => {
                output.report(&format!("established in {mode:?} mode"));
            }
            Notification::RetryQuestion {
                operation, detail, ..
            } => {
                output.report(&format!(
                    "asked how long to wait after a failed {operation:?} ({detail:?}); answering {OUR_RETRY_DELAY:?}"
                ));
                session.answer(watch.id(), Some(OUR_RETRY_DELAY));
            }
            Notification::Completion { outcome, .. } => {
                output.report(&format!("registration: {outcome:?}"));
            }
            Notification::VolumeChanged {
                previous, current, ..
            } => {
                output.report(&format!(
                    "reopened on a different volume ({:?} -> {:?}); continuing",
                    previous.volume_label(),
                    current.volume_label()
                ));
                session.answer_volume_change(watch.id(), VolumeChangeDecision::Continue);
            }
        }
    }

    drop(watch);
    ExitCode::SUCCESS
}
