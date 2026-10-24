// Copyright (c) 2026 Mike Grier

use super::{shared_in_or, shared_or};
use crate::IoFault;
use core::error::Error;
use rc_errors::{ErrorReport, Refusing, StaticError};

static FALLBACK: StaticError = StaticError::new("the report could not be allocated");

/// Refuses every allocation, so the fallback is reached.
static REFUSING: Refusing = Refusing;

#[test]
fn a_shared_report_holds_the_error_it_was_given() {
    let report = shared_or(StaticError::new("the flush failed"), &FALLBACK);
    assert!(matches!(report, ErrorReport::Shared(_)));
    assert_eq!(report.to_string(), "the flush failed");
    assert!(report.downcast_ref::<StaticError>().is_some());
}

#[test]
fn each_shared_report_is_its_own_error() {
    let first = shared_or(StaticError::new("same"), &FALLBACK);
    let second = shared_or(StaticError::new("same"), &FALLBACK);
    assert_ne!(first, second);
    assert_eq!(first, first.clone());
}

#[test]
fn a_failed_allocation_falls_back_to_the_static_report() {
    let report = shared_in_or(StaticError::new("lost"), &REFUSING, &FALLBACK);
    assert_eq!(report, ErrorReport::from_static(&FALLBACK));
    assert_eq!(report.to_string(), "the report could not be allocated");
}

#[test]
fn an_io_error_is_reported_and_found_again_by_downcasting() {
    let report = crate::report_io_error(std::io::Error::from_raw_os_error(5));
    let found = report
        .downcast_ref::<std::io::Error>()
        .expect("the io::Error");
    assert_eq!(found.raw_os_error(), Some(5));
}

#[test]
fn a_fault_becomes_an_io_error_that_holds_it() {
    let fault = IoFault {
        source: "test",
        code: None,
        error: ErrorReport::from_static(&FALLBACK),
    };
    let error = std::io::Error::from(fault.clone());
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    let inner = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<IoFault>())
        .expect("the fault");
    assert_eq!(*inner, fault);
    assert!(inner.source().is_some());
}

#[test]
fn a_fault_reporting_an_io_error_keeps_its_kind_as_an_io_error() {
    for kind in [
        std::io::ErrorKind::PermissionDenied,
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::TimedOut,
    ] {
        let fault = IoFault {
            source: "test",
            code: None,
            error: crate::report_io_error(std::io::Error::new(kind, "below")),
        };
        let error = std::io::Error::from(fault.clone());
        assert_eq!(error.kind(), kind);
        assert_eq!(
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<IoFault>()),
            Some(&fault)
        );
    }
}
