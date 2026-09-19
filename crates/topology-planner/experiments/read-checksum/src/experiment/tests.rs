// Copyright (c) Mike Grier.
use super::*;

#[test]
fn budget_rejects_expiry_and_cancellation_but_accepts_live_work() {
    let cancel = AtomicBool::new(false);
    let mut budget = Budget {
        deadline: Instant::now() + Duration::from_secs(1),
        cancel: &cancel,
    };
    assert!(budget.check().is_ok());
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        budget.check().unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    cancel.store(false, Ordering::Relaxed);
    budget.deadline = Instant::now();
    assert_eq!(budget.check().unwrap_err().kind(), io::ErrorKind::TimedOut);
}

#[test]
fn failure_guard_cancels_on_failure_and_unwind_not_success() {
    let cancel = AtomicBool::new(false);
    drop(CancelOnFailure {
        cancel: &cancel,
        armed: false,
    });
    assert!(!cancel.load(Ordering::Relaxed));
    drop(CancelOnFailure {
        cancel: &cancel,
        armed: true,
    });
    assert!(cancel.load(Ordering::Relaxed));
    cancel.store(false, Ordering::Relaxed);
    let result = std::panic::catch_unwind(|| {
        let _failure = CancelOnFailure {
            cancel: &cancel,
            armed: true,
        };
        panic!("injected worker failure");
    });
    assert!(result.is_err());
    assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn checksum_budget_failure_propagates() {
    let mut checks = 0;
    let result = checksum_checked(&[0; 1024], 5, || {
        checks += 1;
        if checks == 3 {
            Err(failed("injected checksum failure"))
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert_eq!(checks, 3);
}
