// Copyright (c) 2026 Mike Grier
//! Integration tests for D-79 (M10): every fault/failure report carries a real
//! `FaultDetail`, not just which operation or classification was involved.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_file_watcher::{
    FailureCode, FaultDetail, FaultOperation, Monitor, Notification, OpenFailure, Outcome,
    Receiver, RetryMode, WatchOptions,
};

/// Upper bound for waiting on something the monitor really should deliver.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// A uniquely named temp path, removed when the test passes.
struct TempPath {
    path: PathBuf,
}

impl TempPath {
    /// A fresh, non-existent path under the system temp directory.
    fn reserve(label: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "windows-file-watcher-it-{label}-{}-{nonce}",
            std::process::id()
        ));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Drain `receiver` until `predicate` holds over everything seen so far,
/// failing rather than hanging.
fn drain_until<F>(receiver: &Receiver, what: &str, mut predicate: F) -> Vec<Notification>
where
    F: FnMut(&[Notification]) -> bool,
{
    let deadline = Instant::now() + NOTIFY_TIMEOUT;
    let mut seen = Vec::new();
    loop {
        while let Some(item) = receiver.try_recv() {
            seen.push(item);
        }
        if predicate(&seen) {
            return seen;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; saw {} notification(s)",
            seen.len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_permanent_open_failure_reports_its_real_failure_code() {
    // An interior NUL is rejected before any syscall (D-22's `InvalidPath`,
    // the crate's own synthetic `ERROR_INVALID_NAME`), so the expected detail
    // is deterministic. `NotADirectory`, the checklist's other named permanent
    // failure, turns out to be unreachable through `subscribe` in practice: a
    // path that opens as a non-directory is always retried as a file target
    // (D-7) against its real, existing parent directory, which succeeds.
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe("c:\\some\0path", WatchOptions::new())
        .expect("register");

    let seen = drain_until(&receiver, "the permanent failure", |seen| {
        seen.iter().any(|item| {
            matches!(
                item,
                Notification::Completion {
                    watch: tag,
                    ..
                } if *tag == watch.id()
            )
        })
    });

    let outcome = seen
        .iter()
        .find_map(|item| match item {
            Notification::Completion {
                watch: tag,
                outcome,
            } if *tag == watch.id() => Some(*outcome),
            _ => None,
        })
        .expect("a completion for this subscription");

    assert_eq!(
        outcome,
        Outcome::Failed {
            detail: FaultDetail {
                failure: OpenFailure::InvalidPath,
                code: FailureCode::Win32(windows_sys::Win32::Foundation::ERROR_INVALID_NAME),
            }
        }
    );

    drop(watch);
    drop(monitor);
}

#[test]
fn an_interactive_retry_question_for_a_retryable_open_failure_reports_a_real_failure_code() {
    // The path does not exist yet, which is D-22's retryable case: the monitor
    // parks the subscription and, being interactive, asks how long to wait.
    let target = TempPath::reserve("not-yet-created");

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(
            target.path(),
            WatchOptions::new().retry(RetryMode::Interactive),
        )
        .expect("register");

    let seen = drain_until(&receiver, "the retry question", |seen| {
        seen.iter()
            .any(|item| matches!(item, Notification::RetryQuestion { watch: tag, .. } if *tag == watch.id()))
    });

    let (operation, detail) = seen
        .iter()
        .find_map(|item| match item {
            Notification::RetryQuestion {
                watch: tag,
                operation,
                detail,
            } if *tag == watch.id() => Some((*operation, *detail)),
            _ => None,
        })
        .expect("a retry question for this subscription");

    assert_eq!(operation, FaultOperation::Open);
    assert_eq!(
        detail.failure,
        OpenFailure::NotFound,
        "a target that does not exist yet classifies as NotFound"
    );
    assert!(
        matches!(detail.code, FailureCode::Win32(code) if code != 0),
        "a real OS error code should be carried, saw {:?}",
        detail.code
    );

    session.answer(watch.id(), Some(Duration::from_millis(1)));
    drop(watch);
    drop(monitor);
    target.cleanup();
}
