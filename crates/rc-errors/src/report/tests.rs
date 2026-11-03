// Copyright (c) 2026 Mike Grier

use super::ErrorReport;
use crate::shared::SharedError;
use crate::static_error::StaticError;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;
use core::error::Error;
use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

static REFUSED: StaticError = StaticError::new("refused");
static ALSO_REFUSED: StaticError = StaticError::new("refused");
static BUSY: StaticError = StaticError::new("busy");

const fn assert_traits<T: Clone + Eq + Send + Sync + fmt::Debug + 'static>() {}
const _: () = assert_traits::<ErrorReport>();
const FROM_A_CONST: ErrorReport = ErrorReport::from_static(&BUSY);

fn counter() -> &'static AtomicUsize {
    Box::leak(Box::new(AtomicUsize::new(0)))
}

#[derive(Debug)]
struct Counted {
    drops: &'static AtomicUsize,
}

impl fmt::Display for Counted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("counted")
    }
}

impl Error for Counted {}

impl Drop for Counted {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct WithSource;

impl fmt::Display for WithSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("with a source")
    }
}

impl Error for WithSource {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&REFUSED)
    }
}

static WITH_SOURCE: WithSource = WithSource;

fn shared<E: Error + Send + Sync + 'static>(error: E) -> SharedError {
    SharedError::try_new(error)
        .map_err(|(_, cause)| cause)
        .expect("the block")
}

#[test]
fn a_static_report_reads_as_its_static() {
    let report = ErrorReport::from_static(&REFUSED);
    assert_eq!(report.to_string(), "refused");
    assert_eq!(
        format!("{report:?}"),
        "StaticError { message: \"refused\" }"
    );
    assert_eq!(FROM_A_CONST.to_string(), "busy");
}

#[test]
fn a_shared_report_reads_as_its_error() {
    let report = ErrorReport::from(shared(StaticError::new("shared")));
    assert_eq!(report.to_string(), "shared");
    assert_eq!(format!("{report:?}"), "StaticError { message: \"shared\" }");
}

#[test]
fn a_static_report_equals_another_of_the_same_static_only() {
    let report = ErrorReport::from_static(&REFUSED);
    assert_eq!(report, report.clone());
    assert_eq!(report, ErrorReport::from_static(&REFUSED));
    assert_ne!(
        report,
        ErrorReport::from_static(&ALSO_REFUSED),
        "same message, another static"
    );
    assert_ne!(report, ErrorReport::from_static(&BUSY));
}

#[test]
fn a_shared_report_equals_another_of_the_same_block_only() {
    let error = shared(StaticError::new("shared"));
    let report = ErrorReport::Shared(error.clone());
    assert_eq!(report, report.clone());
    assert_eq!(report, ErrorReport::Shared(error));
    assert_ne!(
        report,
        ErrorReport::Shared(shared(StaticError::new("shared")))
    );
}

#[test]
fn a_static_report_never_equals_a_shared_one() {
    let as_static = ErrorReport::from_static(&REFUSED);
    let as_shared = ErrorReport::Shared(shared(REFUSED));
    assert_ne!(as_static, as_shared);
    assert_ne!(as_shared, as_static);
    assert_eq!(as_static.to_string(), as_shared.to_string());
}

#[test]
fn both_forms_downcast_to_their_concrete_error() {
    let as_static = ErrorReport::from_static(&REFUSED);
    assert_eq!(as_static.downcast_ref::<StaticError>(), Some(&REFUSED));
    let as_shared = ErrorReport::Shared(shared(std::io::Error::other("gone")));
    assert!(as_shared.downcast_ref::<std::io::Error>().is_some());
    assert!(as_shared.downcast_ref::<StaticError>().is_none());
}

#[test]
fn both_forms_report_their_errors_source() {
    let as_static = ErrorReport::from_static(&WITH_SOURCE);
    let source = Error::source(&as_static).expect("a source");
    assert_eq!(source.to_string(), "refused");
    let as_shared = ErrorReport::Shared(shared(WithSource));
    assert_eq!(
        Error::source(&as_shared).expect("a source").to_string(),
        "refused"
    );
    assert!(Error::source(&ErrorReport::from_static(&BUSY)).is_none());
}

#[test]
fn it_converts_from_either_form() {
    let error: &'static (dyn Error + Send + Sync) = &BUSY;
    assert_eq!(ErrorReport::from(error), ErrorReport::from_static(&BUSY));
    let shared_error = shared(StaticError::new("converted"));
    assert_eq!(
        ErrorReport::from(shared_error.clone()),
        ErrorReport::Shared(shared_error)
    );
}

#[test]
fn dropping_the_last_shared_report_drops_the_error() {
    let drops = counter();
    let report = ErrorReport::Shared(shared(Counted { drops }));
    let clone = report.clone();
    drop(report);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(clone.to_string(), "counted");
    drop(clone);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn it_is_itself_an_error_that_can_be_boxed() {
    let boxed: Box<dyn Error + Send + Sync> = Box::new(ErrorReport::from_static(&REFUSED));
    assert_eq!(boxed.to_string(), "refused");
    assert!(boxed.downcast_ref::<ErrorReport>().is_some());
}

#[test]
fn a_report_crosses_threads() {
    let report = ErrorReport::Shared(shared(StaticError::new("threaded")));
    let elsewhere = report.clone();
    let text = std::thread::spawn(move || elsewhere.to_string())
        .join()
        .expect("the other thread");
    assert_eq!(text, report.to_string());
}
