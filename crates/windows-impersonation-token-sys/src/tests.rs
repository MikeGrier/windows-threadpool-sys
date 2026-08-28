// Copyright (c) 2026 Mike Grier

use std::any::Any;
use std::cell::Cell;
use std::error::Error as _;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_CANT_OPEN_ANONYMOUS, ERROR_NO_TOKEN, FALSE,
};

use super::{
    ApplicationGuard, ApplyFailure, CaptureFailure, ThreadTokenOpenError, check_application_result,
    classify_thread_token_open_error, restore, run_in_scope,
};

struct DropFlag {
    drops: Rc<Cell<usize>>,
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(i32::try_from(code).expect("test error code fits in i32"))
}

#[test]
fn no_thread_token_is_classified_as_process_context() {
    let error = os_error(ERROR_NO_TOKEN);
    assert!(matches!(
        classify_thread_token_open_error(error),
        ThreadTokenOpenError::NoToken
    ));
}

#[test]
fn anonymous_thread_token_is_a_typed_capture_failure() {
    let error = os_error(ERROR_CANT_OPEN_ANONYMOUS);
    let ThreadTokenOpenError::Capture(error) = classify_thread_token_open_error(error) else {
        panic!("anonymous context must be a capture failure");
    };

    assert_eq!(error.failure(), CaptureFailure::AnonymousContext);
    assert_eq!(
        error.raw_os_error(),
        Some(i32::try_from(ERROR_CANT_OPEN_ANONYMOUS).expect("error code fits in i32"))
    );
    assert!(error.source().is_some());
}

#[test]
fn other_thread_token_open_errors_keep_their_stage_and_code() {
    let error = os_error(ERROR_ACCESS_DENIED);
    let ThreadTokenOpenError::Capture(error) = classify_thread_token_open_error(error) else {
        panic!("access denied must be a capture failure");
    };

    assert_eq!(error.failure(), CaptureFailure::OpenThreadToken);
    assert_eq!(
        error.raw_os_error(),
        Some(i32::try_from(ERROR_ACCESS_DENIED).expect("error code fits in i32"))
    );
    assert!(error.to_string().starts_with("OpenThreadToken:"));
}

#[test]
fn application_failure_does_not_run_the_operation() {
    let called = Cell::new(false);
    let application = check_application_result(FALSE, || os_error(ERROR_ACCESS_DENIED));

    let result = run_in_scope(application, || called.set(true));
    let error = result.expect_err("application failure must be returned");

    assert!(!called.get());
    assert_eq!(error.failure(), ApplyFailure::ApplyToken);
    assert_eq!(
        error.raw_os_error(),
        Some(i32::try_from(ERROR_ACCESS_DENIED).expect("error code fits in i32"))
    );
    assert!(error.source().is_some());
}

#[test]
fn successful_scope_returns_the_operation_value_and_drops_once() {
    let drops = Rc::new(Cell::new(0));
    let inside_drops = Rc::clone(&drops);
    let guard = DropFlag {
        drops: Rc::clone(&drops),
    };

    let value = run_in_scope(Ok(guard), || {
        assert_eq!(inside_drops.get(), 0);
        42
    })
    .expect("mock application succeeds");

    assert_eq!(value, 42);
    assert_eq!(drops.get(), 1);
}

#[test]
fn fallible_operation_result_is_preserved_without_interpretation() {
    let drops = Rc::new(Cell::new(0));
    let guard = DropFlag {
        drops: Rc::clone(&drops),
    };

    let result = run_in_scope(Ok(guard), || Err::<(), _>("operation failed"))
        .expect("mock application succeeds");

    assert_eq!(result, Err("operation failed"));
    assert_eq!(drops.get(), 1);
}

#[test]
fn unwinding_operation_drops_the_scope_once() {
    let drops = Rc::new(Cell::new(0));
    let guard = DropFlag {
        drops: Rc::clone(&drops),
    };

    let unwind = catch_unwind(AssertUnwindSafe(|| {
        let _ = run_in_scope(Ok(guard), || -> () { panic!("operation panic") });
    }));

    assert!(unwind.is_err());
    assert_eq!(drops.get(), 1);
}

#[test]
fn captured_token_type_is_send_sync_and_clone() {
    fn assert_traits<T: Send + Sync + Clone>() {}
    assert_traits::<super::ImpersonationToken>();
}

#[test]
fn application_guard_is_neither_send_nor_sync() {
    trait AmbiguousIfSend<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}

    trait AmbiguousIfSync<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfSync<()> for T {}
    impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}

    let _ = <ApplicationGuard as AmbiguousIfSend<_>>::marker;
    let _ = <ApplicationGuard as AmbiguousIfSync<_>>::marker;
}

/// Restoration-failure panic and double-panic process behavior.
///
/// Moved from an integration test (`tests/restoration_failure.rs`) that
/// pulled `src/restore.rs` in via `#[path]` to call `restore::panic_failure`
/// -- which duplicated the module rather than exercising the crate's real
/// compiled module graph, and could silently stop testing the real thing if
/// the crate ever stopped calling that helper the way these tests assume.
/// As unit tests, both call it directly with no such gap.
const RESTORE_FAILURE_SCENARIO_VAR: &str = "WITS_RESTORE_FAILURE_SCENARIO";
const DOUBLE_PANIC_SCENARIO: &str = "double-panic";
const RESTORE_FAILURE_CHILD_TIMEOUT: Duration = Duration::from_secs(30);
const RESTORE_FAILURE_SETUP_FAILURE_EXIT_CODE: i32 = 111;

struct RestorationFailure;

impl Drop for RestorationFailure {
    fn drop(&mut self) {
        restore::panic_failure(os_error(ERROR_ACCESS_DENIED));
    }
}

fn restoration_failure_panic_text(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

fn restoration_failure_child_double_panics() -> ! {
    let _restoration = RestorationFailure;
    panic!("operation panic");
}

fn restoration_failure_dispatch_if_child() {
    let Ok(scenario) = std::env::var(RESTORE_FAILURE_SCENARIO_VAR) else {
        return;
    };

    let caught = catch_unwind(|| match scenario.as_str() {
        DOUBLE_PANIC_SCENARIO => restoration_failure_child_double_panics(),
        other => panic!("unknown child scenario {other}"),
    });
    if caught.is_err() {
        std::process::exit(RESTORE_FAILURE_SETUP_FAILURE_EXIT_CODE);
    }
}

#[test]
fn restoration_failure_panics_with_the_native_error() {
    let panic = catch_unwind(|| {
        restore::panic_failure(os_error(ERROR_ACCESS_DENIED));
    })
    .expect_err("restoration failure must panic");
    let text = restoration_failure_panic_text(panic.as_ref()).expect("panic payload is text");

    assert!(text.contains("SetThreadToken failed to restore the previous thread token"));
    assert!(text.contains("os error 5"));
}

#[test]
fn restoration_failure_during_unwind_aborts_the_process() {
    restoration_failure_dispatch_if_child();

    let executable = std::env::current_exe().expect("locate the unit-test binary");
    let mut child = Command::new(executable)
        .arg("--exact")
        .arg("tests::restoration_failure_during_unwind_aborts_the_process")
        .env(RESTORE_FAILURE_SCENARIO_VAR, DOUBLE_PANIC_SCENARIO)
        .env("RUST_TEST_THREADS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn double-panic child");
    let deadline = Instant::now() + RESTORE_FAILURE_CHILD_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll double-panic child") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("double-panic child did not exit within {RESTORE_FAILURE_CHILD_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(
        !status.success(),
        "double-panic child exited successfully instead of aborting"
    );
    assert_ne!(
        status.code(),
        Some(RESTORE_FAILURE_SETUP_FAILURE_EXIT_CODE),
        "double-panic child failed during setup instead of aborting"
    );
}
