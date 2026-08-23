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

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: fault_recovery <directory>");
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

    println!("watching {path}; press Ctrl+C to stop");

    while let Some(notification) = receiver.recv() {
        match notification {
            Notification::Batch { changes, .. } => {
                println!("{} change(s)", changes.len());
            }
            Notification::Desync { cause, .. } => {
                println!("desync ({cause:?}): re-scan the directory");
            }
            Notification::Suspended { .. } => {
                println!("suspended: the monitor is working to re-establish the watch");
            }
            Notification::Resumed { .. } => {
                println!("resumed: delivering again");
            }
            Notification::Established { mode, .. } => {
                println!("established in {mode:?} mode");
            }
            Notification::RetryQuestion {
                operation, detail, ..
            } => {
                println!(
                    "asked how long to wait after a failed {operation:?} ({detail:?}); answering {OUR_RETRY_DELAY:?}"
                );
                session.answer(watch.id(), Some(OUR_RETRY_DELAY));
            }
            Notification::Completion { outcome, .. } => {
                println!("registration: {outcome:?}");
            }
            Notification::VolumeChanged {
                previous, current, ..
            } => {
                println!(
                    "reopened on a different volume ({:?} -> {:?}); continuing",
                    previous.volume_label(),
                    current.volume_label()
                );
                session.answer_volume_change(watch.id(), VolumeChangeDecision::Continue);
            }
        }
    }

    drop(watch);
    ExitCode::SUCCESS
}
