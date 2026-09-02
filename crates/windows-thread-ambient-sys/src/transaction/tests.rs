// Copyright (c) Mike Grier.

//! Tests for the TxF transaction aspect.
//!
//! A real transaction is created through the same lazy `ktmw32` loader the
//! aspect uses, so the tests exercise the `Present` path rather than only the
//! empty one. Where TxF is unavailable -- it is deprecated, and a filesystem or
//! future release may not offer it -- the transaction-creating tests skip
//! explicitly and say so, rather than passing silently on a machine that
//! measured nothing.

use std::error::Error as _;
use std::io;
use std::marker::PhantomData;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

use super::{
    Captured, TransactionContext, TransactionError, TransactionFailure, TransactionGuard, capture,
    is_supported, system_proc, with_applied,
};
use crate::test_injection::{self, FaultPoint};

type CreateTransactionFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    u32,
    u32,
    u32,
    u32,
    *const u16,
) -> HANDLE;
type RollbackTransactionFn = unsafe extern "system" fn(HANDLE) -> i32;

/// A transaction that rolls itself back.
struct Transaction(HANDLE);

impl Transaction {
    /// Create one, or `None` if this system cannot.
    fn new() -> Option<Self> {
        let create = system_proc("ktmw32.dll", b"CreateTransaction\0")?;
        // SAFETY: transmuted to the signature ktmw32.h declares.
        let create: CreateTransactionFn = unsafe { std::mem::transmute(create) };
        // SAFETY: every pointer argument is null, which the API accepts, and the
        // numeric arguments are the documented defaults.
        let handle = unsafe {
            create(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                std::ptr::null(),
            )
        };
        if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return None;
        }
        Some(Self(handle))
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if let Some(rollback) = system_proc("ktmw32.dll", b"RollbackTransaction\0") {
            // SAFETY: transmuted to the declared signature; `self.0` is live.
            let rollback: RollbackTransactionFn = unsafe { std::mem::transmute(rollback) };
            // SAFETY: `self.0` is a live transaction this type owns.
            unsafe { rollback(self.0) };
        }
        // SAFETY: nothing else owns this handle.
        unsafe { CloseHandle(self.0) };
    }
}

/// Install a transaction on this thread for the duration of `body`.
fn while_transacted<T>(transaction: &Transaction, body: impl FnOnce() -> T) -> T {
    super::set_current(transaction.0).expect("installing a transaction");
    let outcome = body();
    super::set_current(std::ptr::null_mut()).expect("clearing the transaction");
    outcome
}

/// The thread's current transaction, as the aspect sees it.
fn live() -> HANDLE {
    super::current_raw().expect("the entry points resolved")
}

#[test]
fn the_entry_points_resolve_on_this_system() {
    // If this fails, every other transaction test below is meaningless, so it is
    // asserted rather than assumed by the tests that depend on it.
    assert!(
        is_supported(),
        "ktmw32.dll did not offer the thread-transaction entry points"
    );
}

#[test]
fn support_probe_reports_an_injected_missing_platform() {
    let _faults = test_injection::fail(&[(FaultPoint::TransactionSupport, 1)]);

    assert!(!is_supported());
    assert_eq!(test_injection::calls(FaultPoint::TransactionSupport), 1);
}

#[test]
fn transaction_errors_preserve_failure_code_display_and_source() {
    const CODE: i32 = 1234;
    for (failure, description) in [
        (
            TransactionFailure::Unsupported,
            "ktmw32.dll does not offer the thread-transaction entry points",
        ),
        (
            TransactionFailure::Duplicate,
            "the transaction handle could not be duplicated",
        ),
        (
            TransactionFailure::Install,
            "the thread transaction could not be set",
        ),
    ] {
        let error = TransactionError {
            failure,
            source: Some(io::Error::from_raw_os_error(CODE)),
        };

        assert_eq!(error.failure(), failure);
        assert_eq!(error.raw_os_error(), Some(CODE));
        assert!(error.to_string().starts_with(description));
        assert_eq!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<io::Error>())
                .and_then(io::Error::raw_os_error),
            Some(CODE)
        );
    }
}

#[test]
fn unsupported_transaction_errors_can_have_no_os_source() {
    let error = TransactionError {
        failure: TransactionFailure::Unsupported,
        source: None,
    };

    assert_eq!(error.raw_os_error(), None);
    assert!(error.source().is_none());
    assert_eq!(
        error.to_string(),
        "ktmw32.dll does not offer the thread-transaction entry points"
    );
}

#[test]
fn explicit_release_reports_an_injected_restore_failure() {
    let _faults = test_injection::fail(&[(FaultPoint::TransactionSet, 1)]);
    let guard = TransactionGuard {
        previous: Some(std::ptr::null_mut()),
        released: false,
        captured: PhantomData,
    };

    let error = guard
        .release()
        .expect_err("explicit release must report restore failure");
    assert_eq!(error.failure(), TransactionFailure::Install);
    assert_eq!(test_injection::calls(FaultPoint::TransactionSet), 1);
}

#[test]
fn dropping_an_unreleased_guard_attempts_best_effort_restore() {
    let _faults = test_injection::fail(&[(FaultPoint::TransactionSet, 1)]);
    drop(TransactionGuard {
        previous: Some(std::ptr::null_mut()),
        released: false,
        captured: PhantomData,
    });

    assert_eq!(test_injection::calls(FaultPoint::TransactionSet), 1);
}

#[test]
fn dropping_a_released_guard_does_not_restore_twice() {
    let _faults = test_injection::fail(&[(FaultPoint::TransactionSet, 1)]);
    drop(TransactionGuard {
        previous: Some(std::ptr::null_mut()),
        released: true,
        captured: PhantomData,
    });

    assert_eq!(test_injection::calls(FaultPoint::TransactionSet), 0);
}

#[test]
fn a_thread_without_a_transaction_captures_as_absent() {
    assert!(
        super::is_none_sentinel(live()),
        "precondition: no transaction"
    );
    let captured = capture().expect("capture succeeds");
    assert!(
        matches!(captured, Captured::Absent),
        "an untransacted thread should capture as Absent, not NotCaptured or Present"
    );
}

#[test]
fn not_captured_leaves_the_threads_own_transaction_alone() {
    let Some(transaction) = Transaction::new() else {
        eprintln!("skipped: this system cannot create a transaction");
        return;
    };
    while_transacted(&transaction, || {
        let before = live();
        let value = with_applied(&Captured::NotCaptured, || {
            assert_eq!(live(), before, "NotCaptured disturbed the thread");
            7
        })
        .expect("nothing to install");
        assert_eq!(value, 7);
        assert_eq!(live(), before);
    });
}

#[test]
fn absent_clears_the_threads_transaction_for_the_operation() {
    // The distinction NotCaptured and Absent exist to express: the caller asked
    // and had none, so the worker must not silently enlist in one of its own.
    let Some(transaction) = Transaction::new() else {
        eprintln!("skipped: this system cannot create a transaction");
        return;
    };
    while_transacted(&transaction, || {
        let before = live();
        assert!(!super::is_none_sentinel(before), "precondition: transacted");
        let cleared = with_applied(&Captured::Absent, live).expect("apply absent");
        assert!(
            super::is_none_sentinel(cleared),
            "Absent did not clear the thread's transaction"
        );
        assert_eq!(live(), before, "the entry transaction was not restored");
    });
}

#[test]
fn a_captured_transaction_is_installed_and_restored() {
    let Some(transaction) = Transaction::new() else {
        eprintln!("skipped: this system cannot create a transaction");
        return;
    };
    let captured = while_transacted(&transaction, || {
        capture().expect("capture while transacted")
    });
    assert!(matches!(captured, Captured::Present(_)));

    assert!(
        super::is_none_sentinel(live()),
        "precondition: untransacted"
    );
    let during = with_applied(&captured, live).expect("apply");
    assert!(
        !super::is_none_sentinel(during),
        "the transaction was not installed"
    );
    assert!(
        super::is_none_sentinel(live()),
        "the thread was left transacted"
    );
}

#[test]
fn the_captured_handle_is_a_duplicate_not_the_originals_value() {
    // Ownership is what lets a request outlive its originator's handle, so the
    // duplicate must be a distinct handle value rather than the same one.
    let Some(transaction) = Transaction::new() else {
        eprintln!("skipped: this system cannot create a transaction");
        return;
    };
    let original = transaction.0;
    let captured = while_transacted(&transaction, || capture().expect("capture"));
    let Captured::Present(context) = &captured else {
        panic!("expected a present transaction");
    };
    assert_ne!(
        context.as_raw(),
        original,
        "capture returned the caller's own handle rather than a duplicate"
    );
}

#[test]
fn a_captured_transaction_applies_on_another_thread() {
    let Some(transaction) = Transaction::new() else {
        eprintln!("skipped: this system cannot create a transaction");
        return;
    };
    let captured = while_transacted(&transaction, || capture().expect("capture"));
    // A raw HANDLE is not `Send`, so the worker reports predicates rather than
    // handles. That is the crate's own rule applied to its tests: what crosses a
    // thread boundary is an owned value, never a raw reference.
    let observed = std::thread::spawn(move || {
        let inherited = super::is_none_sentinel(live());
        let during = with_applied(&captured, || super::is_none_sentinel(live()))
            .expect("apply on the worker");
        (inherited, during, super::is_none_sentinel(live()))
    })
    .join()
    .expect("the worker did not panic");

    assert!(observed.0, "a fresh worker should carry no transaction");
    assert!(!observed.1, "the transaction did not reach the worker");
    assert!(observed.2, "the worker was left transacted");
}

#[test]
fn the_captured_value_outlives_the_originating_handle() {
    // The lifetime half of the ownership decision, stated as a test: the
    // duplicate must remain usable after the caller's handle is gone.
    let Some(transaction) = Transaction::new() else {
        eprintln!("skipped: this system cannot create a transaction");
        return;
    };
    let captured = while_transacted(&transaction, || capture().expect("capture"));
    drop(transaction);

    let during = with_applied(&captured, live).expect("apply after the original closed");
    assert!(
        !super::is_none_sentinel(during),
        "the duplicate did not survive its originating handle"
    );
}

#[test]
fn the_operations_return_value_is_passed_through() {
    let value = with_applied(&Captured::NotCaptured, || String::from("carried"))
        .expect("nothing to install");
    assert_eq!(value, "carried");
}

#[test]
fn an_unsupported_failure_is_distinct_from_absence() {
    // The two are different facts and the type keeps them apart; this asserts
    // the discriminants exist rather than exercising an unavailable platform.
    assert_ne!(TransactionFailure::Unsupported, TransactionFailure::Install);
    assert_ne!(
        TransactionFailure::Unsupported,
        TransactionFailure::Duplicate
    );
}

#[test]
fn a_context_is_send_so_it_can_reach_a_worker() {
    fn assert_send<T: Send>() {}
    assert_send::<TransactionContext>();
    assert_send::<Captured<TransactionContext>>();
}
