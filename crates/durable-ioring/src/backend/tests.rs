// Copyright (c) 2026 Mike Grier
//! What a refusal cause tells its caller to do.

use rc_errors::{ErrorReport, StaticError};

use super::{RefusalCause, RefusalKind};
use crate::types::IoFault;

static TOLD: StaticError = StaticError::new("told");

fn fault() -> IoFault {
    IoFault {
        source: "test",
        code: None,
        refusal: None,
        error: ErrorReport::from_static(&TOLD),
    }
}

/// Every cause, with whether it needs a submit, a drain, and whether it is retryable.
fn table() -> Vec<(RefusalCause, bool, bool, bool)> {
    vec![
        (RefusalCause::SubmitNeeded, true, false, true),
        (RefusalCause::DrainNeeded, false, true, true),
        (RefusalCause::SubmitAndDrainNeeded, true, true, true),
        (RefusalCause::Busy, false, false, true),
        (RefusalCause::UnknownTarget, false, false, false),
        (RefusalCause::Unsupported, false, false, false),
        (RefusalCause::Invalid, false, false, false),
        (RefusalCause::AccessDenied, false, false, false),
        (RefusalCause::Unusable(fault()), false, false, false),
        (RefusalCause::Unknown(fault()), false, false, false),
    ]
}

#[test]
fn each_cause_names_the_actions_that_clear_it() {
    for (cause, submit, drain, retryable) in table() {
        assert_eq!(cause.needs_submit(), submit, "{cause:?} needs_submit");
        assert_eq!(cause.needs_drain(), drain, "{cause:?} needs_drain");
        assert_eq!(cause.is_retryable(), retryable, "{cause:?} is_retryable");
    }
}

#[test]
fn every_cause_displays_distinctly() {
    let shown: Vec<String> = table()
        .iter()
        .map(|(cause, ..)| cause.to_string())
        .collect();
    for (i, a) in shown.iter().enumerate() {
        assert!(!a.is_empty());
        for b in &shown[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn a_cause_carrying_a_fault_shows_what_the_backend_said() {
    assert!(
        RefusalCause::Unusable(fault())
            .to_string()
            .ends_with("told")
    );
    assert!(RefusalCause::Unknown(fault()).to_string().ends_with("told"));
}

/// A report shared from the global allocator, as a backend that allocates its reports gives one.
fn shared_report() -> ErrorReport {
    static UNSHARED: StaticError = StaticError::new("unshared");
    crate::fault_report::shared_or(StaticError::new("shared"), &UNSHARED)
}

#[test]
fn every_cause_has_its_own_kind() {
    let kinds = [
        (RefusalCause::SubmitNeeded, RefusalKind::SubmitNeeded),
        (RefusalCause::DrainNeeded, RefusalKind::DrainNeeded),
        (
            RefusalCause::SubmitAndDrainNeeded,
            RefusalKind::SubmitAndDrainNeeded,
        ),
        (RefusalCause::Busy, RefusalKind::Busy),
        (RefusalCause::UnknownTarget, RefusalKind::UnknownTarget),
        (RefusalCause::Unsupported, RefusalKind::Unsupported),
        (RefusalCause::Invalid, RefusalKind::Invalid),
        (RefusalCause::AccessDenied, RefusalKind::AccessDenied),
        (RefusalCause::Unusable(fault()), RefusalKind::Unusable),
        (RefusalCause::Unknown(fault()), RefusalKind::Unknown),
    ];
    assert_eq!(kinds.len(), table().len());
    for (i, (cause, kind)) in kinds.iter().enumerate() {
        assert_eq!(cause.kind(), *kind, "{cause:?}");
        for (_, other) in &kinds[i + 1..] {
            assert_ne!(kind, other);
        }
    }
}

#[test]
fn a_refusal_fault_names_its_cause_and_carries_the_backends_code() {
    use super::{REFUSED, refusal_fault};
    use crate::error_code::ErrorCode;

    let coded = || IoFault {
        source: "test",
        code: Some(ErrorCode::from_hresult(1117)),
        refusal: None,
        error: ErrorReport::from_static(&TOLD),
    };
    let mut cases: Vec<(RefusalCause, Option<ErrorCode>)> = table()
        .into_iter()
        .map(|(cause, ..)| (cause, None))
        .collect();
    cases.push((
        RefusalCause::Unusable(coded()),
        Some(ErrorCode::from_hresult(1117)),
    ));
    cases.push((
        RefusalCause::Unknown(coded()),
        Some(ErrorCode::from_hresult(1117)),
    ));
    for (cause, code) in cases {
        let fault = refusal_fault(cause.clone());
        assert_eq!(fault.source, REFUSED);
        assert_eq!(fault.code, code, "{cause:?}");
        assert_eq!(fault.refusal, Some(cause.kind()), "{cause:?}");
    }
}

#[test]
fn a_cause_carrying_nothing_is_reported_by_a_static_of_the_cause() {
    use super::refusal_fault;

    for (cause, ..) in table() {
        if matches!(cause, RefusalCause::Unusable(_) | RefusalCause::Unknown(_)) {
            continue;
        }
        let first = refusal_fault(cause.clone());
        let second = refusal_fault(cause.clone());
        assert!(matches!(first.error, ErrorReport::Static(_)), "{cause:?}");
        // A static is one error each time.
        assert_eq!(first, second, "{cause:?}");
        assert_eq!(first.error.to_string(), cause.to_string(), "{cause:?}");
    }
}

#[test]
fn a_cause_carrying_a_fault_passes_the_backends_report_through() {
    use super::refusal_fault;

    let shared = shared_report();
    for reported in [ErrorReport::from_static(&TOLD), shared] {
        for cause in [
            RefusalCause::Unusable(IoFault {
                error: reported.clone(),
                ..fault()
            }),
            RefusalCause::Unknown(IoFault {
                error: reported.clone(),
                ..fault()
            }),
        ] {
            let refused = refusal_fault(cause.clone());
            // The backend's own report, not a new one made to carry the cause.
            assert_eq!(refused.error, reported, "{cause:?}");
            assert!(refused.error.downcast_ref::<RefusalCause>().is_none());
            assert_eq!(refused.refusal, Some(cause.kind()), "{cause:?}");
        }
    }
}

#[test]
fn a_refusal_is_part_of_what_makes_two_faults_equal() {
    let plain = fault();
    let refused = IoFault {
        refusal: Some(RefusalKind::Busy),
        ..fault()
    };
    assert_ne!(plain, refused);
    assert_eq!(refused, refused.clone());
}
