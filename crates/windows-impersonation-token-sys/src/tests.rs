// Copyright (c) 2026 Mike Grier

use std::cell::Cell;
use std::error::Error as _;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_CANT_OPEN_ANONYMOUS, ERROR_NO_TOKEN, FALSE,
};

use super::{
    ApplicationGuard, ApplyFailure, CaptureFailure, ThreadTokenOpenError, check_application_result,
    classify_thread_token_open_error, run_in_scope,
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
