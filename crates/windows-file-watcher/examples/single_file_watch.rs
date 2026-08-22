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

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: single_file_watch <file>");
        return ExitCode::FAILURE;
    };

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(&path, WatchOptions::new())
        .expect("register the subscription");

    println!("watching {path}; press Ctrl+C to stop");

    while let Some(notification) = receiver.recv() {
        match notification {
            Notification::Batch { changes, .. } => {
                for change in changes {
                    println!("{:?}", change.kind);
                }
            }
            Notification::Desync { cause, .. } => {
                println!("desync ({cause:?}): re-check the file's current state");
            }
            Notification::Completion { outcome, .. } => {
                println!("registration: {outcome:?}");
            }
            _ => {}
        }
    }

    drop(watch);
    ExitCode::SUCCESS
}
