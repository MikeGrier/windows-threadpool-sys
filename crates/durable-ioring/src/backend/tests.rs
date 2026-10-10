// Copyright (c) 2026 Mike Grier
//! What a refusal cause tells its caller to do.

use std::io;
use std::sync::Arc;

use super::RefusalCause;
use crate::types::IoFault;

fn fault() -> IoFault {
    IoFault {
        source: "test",
        code: None,
        error: Arc::new(io::Error::other("told")),
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
