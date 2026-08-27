// Copyright (c) 2026 Mike Grier
//! Restoration-failure panic and double-panic process behavior.

#![cfg(windows)]

use std::any::Any;
use std::io;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "../src/restore.rs"]
mod restore;

const SCENARIO_VAR: &str = "WITS_RESTORE_FAILURE_SCENARIO";
const DOUBLE_PANIC_SCENARIO: &str = "double-panic";
const CHILD_TIMEOUT: Duration = Duration::from_secs(30);
const SETUP_FAILURE_EXIT_CODE: i32 = 111;

struct RestorationFailure;

impl Drop for RestorationFailure {
    fn drop(&mut self) {
        restore::panic_failure(io::Error::from_raw_os_error(
            i32::try_from(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED)
                .expect("ERROR_ACCESS_DENIED fits in i32"),
        ));
    }
}

fn panic_text(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

fn child_double_panics() -> ! {
    let _restoration = RestorationFailure;
    panic!("operation panic");
}

fn dispatch_if_child() {
    let Ok(scenario) = std::env::var(SCENARIO_VAR) else {
        return;
    };

    let caught = std::panic::catch_unwind(|| match scenario.as_str() {
        DOUBLE_PANIC_SCENARIO => child_double_panics(),
        other => panic!("unknown child scenario {other}"),
    });
    if caught.is_err() {
        std::process::exit(SETUP_FAILURE_EXIT_CODE);
    }
}

#[test]
fn restoration_failure_panics_with_the_native_error() {
    let panic = std::panic::catch_unwind(|| {
        restore::panic_failure(io::Error::from_raw_os_error(
            i32::try_from(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED)
                .expect("ERROR_ACCESS_DENIED fits in i32"),
        ));
    })
    .expect_err("restoration failure must panic");
    let text = panic_text(panic.as_ref()).expect("panic payload is text");

    assert!(text.contains("SetThreadToken failed to restore the previous thread token"));
    assert!(text.contains("os error 5"));
}

#[test]
fn restoration_failure_during_unwind_aborts_the_process() {
    dispatch_if_child();

    let executable = std::env::current_exe().expect("locate integration-test binary");
    let mut child = Command::new(executable)
        .arg("--exact")
        .arg("restoration_failure_during_unwind_aborts_the_process")
        .env(SCENARIO_VAR, DOUBLE_PANIC_SCENARIO)
        .env("RUST_TEST_THREADS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn double-panic child");
    let deadline = Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll double-panic child") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("double-panic child did not exit within {CHILD_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(
        !status.success(),
        "double-panic child exited successfully instead of aborting"
    );
    assert_ne!(
        status.code(),
        Some(SETUP_FAILURE_EXIT_CODE),
        "double-panic child failed during setup instead of aborting"
    );
}
