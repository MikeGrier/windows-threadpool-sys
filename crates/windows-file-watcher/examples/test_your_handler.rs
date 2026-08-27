// Copyright (c) 2026 Mike Grier
//! Testing your own notification-handling code, with no filesystem and no
//! thread pool.
//!
//! ```text
//! cargo run --example test_your_handler --features test-util
//! ```
//!
//! A consumer of `windows-file-watcher` reacts to [`Notification`]s drained from
//! a `Receiver`. This example shows how to test that reaction logic in
//! isolation: construct a channel yourself, push a scripted, deterministic
//! sequence of synthetic notifications, and assert on what your handler did.
//! This "goes below" the `Monitor` -- you substitute the OS ingest while keeping
//! the real delivery model (`Notification`, `Receiver`, queue ordering). Because
//! the test decides what arrives and when, it is fully deterministic.
//!
//! The channel and the two boundary-type builders (`RelativeName::for_test`,
//! `VolumeIdentity::for_test`) live behind the off-by-default `test-util`
//! feature, which is why this example declares `required-features`.

use std::collections::BTreeSet;
use std::ffi::OsString;

use windows_file_watcher::{
    Change, ChangeKind, DEFAULT_BOUND, DesyncCause, FailureCode, FaultDetail, FaultOperation,
    Notification, OpenFailure, Outcome, RelativeName, VolumeIdentity, WatchId, channel_with_bound,
};

/// The real Win32 error code paired with `OpenFailure::NotFound` below,
/// named per the repo's no-bare-numeric-constants rule.
const ERROR_FILE_NOT_FOUND: u32 = 2;

/// Where this example's reports go, kept as one seam (the repo's
/// architectural pre-step, matching
/// `windows-file-watcher/examples/minimal_directory_watch.rs`) rather than
/// scattering `println!` across the file.
struct Output<O> {
    stdout: O,
}

impl<O: std::io::Write> Output<O> {
    fn report(&mut self, message: &str) {
        let _ = writeln!(self.stdout, "{message}");
    }
}

fn stdio() -> Output<std::io::Stdout> {
    Output {
        stdout: std::io::stdout(),
    }
}

/// The consumer's own state machine -- the thing under test. It reacts to
/// notifications and never talks to the operating system.
#[derive(Default)]
struct Handler {
    /// The set of leaf names currently believed present, maintained from the
    /// change stream (a real consumer's core job). `OsString`, not `String`:
    /// a lossy `to_string_lossy` collapses distinct valid Windows names that
    /// differ only in an unpaired surrogate into the same key.
    present: BTreeSet<OsString>,
    /// How many re-scan (desync) signals were seen.
    rescans: u32,
    /// Whether the subscription was confirmed registered.
    subscribed: bool,
    /// Why the last retry question was asked, if any.
    last_retry_reason: Option<OpenFailure>,
    /// Whether a volume change was observed.
    volume_changed: bool,
}

impl Handler {
    fn on(&mut self, notification: &Notification) {
        match notification {
            Notification::Batch { changes, .. } => {
                for change in changes {
                    let name = change.name.to_os_string();
                    match &change.kind {
                        ChangeKind::Removed | ChangeKind::RenamedOldName => {
                            self.present.remove(&name);
                        }
                        _ => {
                            self.present.insert(name);
                        }
                    }
                }
            }
            Notification::Desync { .. } => self.rescans += 1,
            Notification::Completion { outcome, .. } => {
                if matches!(outcome, Outcome::Subscribed) {
                    self.subscribed = true;
                }
            }
            Notification::RetryQuestion { detail, .. } => {
                self.last_retry_reason = Some(detail.failure);
            }
            Notification::VolumeChanged { .. } => self.volume_changed = true,
            // Suspended / Resumed / Established are opt-in and unused here.
            _ => {}
        }
    }
}

fn main() {
    let (sender, receiver) = channel_with_bound(DEFAULT_BOUND);
    let watch = WatchId::from_raw(1);

    // A scripted, deterministic sequence -- exactly the shape your handler would
    // see in production, but with no real directory and no thread pool.
    let _ = sender.send(Notification::Completion {
        watch,
        outcome: Outcome::Subscribed,
    });
    let _ = sender.send(Notification::Batch {
        watch,
        changes: vec![
            Change {
                kind: ChangeKind::Added,
                name: RelativeName::for_test("report.tmp"),
            },
            Change {
                kind: ChangeKind::RenamedOldName,
                name: RelativeName::for_test("report.tmp"),
            },
            Change {
                kind: ChangeKind::RenamedNewName,
                name: RelativeName::for_test("report.csv"),
            },
        ],
    });
    let _ = sender.send(Notification::Desync {
        watch,
        cause: DesyncCause::Overflow,
    });
    let _ = sender.send(Notification::RetryQuestion {
        watch,
        operation: FaultOperation::Open,
        detail: FaultDetail {
            failure: OpenFailure::NotFound,
            code: FailureCode::Win32(ERROR_FILE_NOT_FOUND),
        },
    });
    let _ = sender.send(Notification::VolumeChanged {
        watch,
        previous: VolumeIdentity::for_test(0x1111, "NTFS", "System"),
        current: VolumeIdentity::for_test(0x2222, "FAT32", "Removable"),
    });

    // Drive the handler exactly as a production loop would: drain and dispatch.
    let mut handler = Handler::default();
    while let Some(notification) = receiver.try_recv() {
        handler.on(&notification);
    }

    // Assert on what the handler did -- deterministic, reproducible, offline.
    assert!(
        handler.subscribed,
        "the Subscribed completion should mark the subscription registered"
    );
    assert!(
        handler.present.contains(std::ffi::OsStr::new("report.csv")),
        "the renamed-in file should be present"
    );
    assert!(
        !handler.present.contains(std::ffi::OsStr::new("report.tmp")),
        "the temp file was renamed away, so it should not be present"
    );
    assert_eq!(handler.rescans, 1, "one Overflow desync means one re-scan");
    assert!(matches!(
        handler.last_retry_reason,
        Some(OpenFailure::NotFound)
    ));
    assert!(handler.volume_changed);

    let mut output = stdio();
    output.report(&format!(
        "handler reacted as expected: {} name(s) present, {} re-scan(s)",
        handler.present.len(),
        handler.rescans
    ));
    output.report("all assertions passed with no filesystem and no thread pool");
}
