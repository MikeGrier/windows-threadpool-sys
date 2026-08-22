// Copyright (c) 2026 Mike Grier
//! Watch a directory recursively and print every change.
//!
//! ```text
//! cargo run --example minimal_directory_watch -- C:\some\directory
//! ```

use std::env;
use std::process::ExitCode;

use windows_file_watcher::{Monitor, Notification, WatchOptions};

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: minimal_directory_watch <directory>");
        return ExitCode::FAILURE;
    };

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(&path, WatchOptions::new().subtree(true))
        .expect("register the subscription");

    println!("watching {path} (recursively); press Ctrl+C to stop");

    while let Some(notification) = receiver.recv() {
        match notification {
            Notification::Batch { changes, .. } => {
                for change in changes {
                    println!("{:?} {}", change.kind, change.name.to_path_buf().display());
                }
            }
            Notification::Desync { cause, .. } => {
                println!("desync ({cause:?}): re-scan the directory");
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
