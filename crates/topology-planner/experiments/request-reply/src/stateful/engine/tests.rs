// Copyright (c) Mike Grier.
use super::*;

fn config(workers: usize, keys: usize, capacity: usize) -> Config {
    Config {
        limits: Limits {
            workers,
            capacity,
            timeout_ms: 1000,
            cancel_after_admitted: None,
        },
        keys,
    }
}

#[test]
fn both_candidates_match_serial_state_over_ten_normal_shapes() {
    for (count, workers, keys, capacity, hot, mix, work) in [
        (0, 1, 1, 1, false, 50, 0),
        (1, 1, 1, 1, true, 100, 1),
        (2, 2, 1, 2, true, 0, 0),
        (7, 3, 2, 3, true, 50, 100),
        (17, 2, 7, 4, false, 100, 10),
        (33, 3, 9, 6, true, 50, 1000),
        (65, 4, 3, 8, true, 75, 256),
        (127, 4, 16, 16, false, 25, 400),
        (128, 8, 17, 16, false, 0, 100),
        (129, 3, 31, 6, true, 100, 1000),
    ] {
        let requests = trace(count, keys, 8, 100, hot, mix, work).unwrap();
        let mut reports = Vec::new();
        for arrangement in [Arrangement::SharedState, Arrangement::KeyOwned] {
            let report = run_with_hooks(
                config(workers, keys, capacity),
                requests.clone(),
                arrangement,
                &AtomicBool::new(false),
                &Hooks {
                    prepare: &prepare,
                    cpu: &|| Ok(0),
                },
            )
            .unwrap();
            assert_eq!(report.stop, StopReason::Drained);
            assert_eq!(report.replies.len(), count);
            assert!(report.unadmitted.is_empty());
            verify(&report).unwrap();
            reports.push(report);
        }
        assert_eq!(reports[0].final_state, reports[1].final_state);
        assert_eq!(
            reports[0]
                .replies
                .iter()
                .map(|reply| (&reply.id, &reply.outcome))
                .collect::<Vec<_>>(),
            reports[1]
                .replies
                .iter()
                .map(|reply| (&reply.id, &reply.outcome))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn cancellation_resolves_admitted_prefix_without_state_after_cancel() {
    for arrangement in [Arrangement::SharedState, Arrangement::KeyOwned] {
        for count in [0, 1, 2, 3, 7, 16, 31] {
            let mut config = config(2, 5, 4);
            config.limits.cancel_after_admitted = Some(count);
            let report = run_with_hooks(
                config,
                trace(32, 5, 32, 0, true, 60, 1000).unwrap(),
                arrangement,
                &AtomicBool::new(false),
                &Hooks {
                    prepare: &prepare,
                    cpu: &|| Ok(0),
                },
            )
            .unwrap();
            assert_eq!(report.stop, StopReason::Cancelled);
            assert_eq!(report.replies.len(), count);
            assert_eq!(report.unadmitted.len(), 32 - count);
            verify(&report).unwrap();
        }
    }
}

#[test]
fn shared_commit_waits_for_an_earlier_key_even_when_preparation_finishes_later() {
    let (prepared_send, prepared_receive) = bounded(1);
    let prepare_out_of_order = move |request: &Request, _: &AtomicBool, _: Instant| {
        match request.id {
            0 => {
                prepared_receive
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap();
            }
            1 => {
                prepared_send.send(()).unwrap();
            }
            _ => {}
        }
        Ok(true)
    };
    let requests = vec![
        Request {
            id: 0,
            key: 0,
            at_ns: 0,
            work: 0,
            operation: Operation::Add { value: 7 },
        },
        Request {
            id: 1,
            key: 0,
            at_ns: 0,
            work: 0,
            operation: Operation::Lookup,
        },
    ];
    let report = run_with_hooks(
        config(2, 1, 4),
        requests,
        Arrangement::SharedState,
        &AtomicBool::new(false),
        &Hooks {
            prepare: &prepare_out_of_order,
            cpu: &|| Ok(0),
        },
    )
    .unwrap();
    assert_eq!(report.final_state, vec![7]);
    assert_eq!(report.replies[1].outcome, Outcome::Completed { value: 7 });
    assert!(report.replies[1].committed_ns >= report.replies[0].committed_ns);
}

#[test]
fn cancellation_and_deadline_before_commit_have_no_effect() {
    for arrangement in [Arrangement::SharedState, Arrangement::KeyOwned] {
        for deadline in [false, true] {
            let mut candidate = config(2, 1, 4);
            candidate.limits.timeout_ms = if deadline { 5 } else { 1000 };
            candidate.limits.cancel_after_admitted = if deadline { None } else { Some(2) };
            let blocked = |_: &Request, cancel: &AtomicBool, expires: Instant| {
                while !cancel.load(Ordering::Acquire) && Instant::now() < expires {
                    thread::yield_now();
                }
                Ok(false)
            };
            let report = run_with_hooks(
                candidate,
                trace(16, 1, 16, 0, true, 100, 0).unwrap(),
                arrangement,
                &AtomicBool::new(false),
                &Hooks {
                    prepare: &blocked,
                    cpu: &|| Ok(0),
                },
            )
            .unwrap();
            assert_eq!(
                report.stop,
                if deadline {
                    StopReason::Deadline
                } else {
                    StopReason::Cancelled
                }
            );
            assert_eq!(report.final_state, vec![0]);
            assert!(
                report
                    .replies
                    .iter()
                    .all(|reply| reply.outcome == Outcome::Cancelled)
            );
            assert_eq!(report.replies.len() + report.unadmitted.len(), 16);
        }
        let report = run_with_hooks(
            config(2, 1, 4),
            trace(8, 1, 8, 0, true, 100, 0).unwrap(),
            arrangement,
            &AtomicBool::new(true),
            &Hooks {
                prepare: &prepare,
                cpu: &|| Ok(0),
            },
        )
        .unwrap();
        assert!(report.replies.is_empty());
        assert_eq!(report.final_state, vec![0]);
    }
}

#[test]
fn injected_worker_failures_do_not_strand_order_waiters() {
    for arrangement in [Arrangement::SharedState, Arrangement::KeyOwned] {
        for panic in [false, true] {
            let broken = move |request: &Request, cancel: &AtomicBool, deadline: Instant| {
                if request.id == 2 {
                    if panic {
                        panic!("injected keyed worker panic");
                    }
                    return Err(failed("injected keyed service failure"));
                }
                prepare(request, cancel, deadline)
            };
            assert!(
                run_with_hooks(
                    config(2, 1, 4),
                    trace(16, 1, 16, 0, true, 50, 100).unwrap(),
                    arrangement,
                    &AtomicBool::new(false),
                    &Hooks {
                        prepare: &broken,
                        cpu: &|| Ok(0)
                    }
                )
                .is_err()
            );
        }
        for fail_at in [0, 2] {
            let samples = std::sync::atomic::AtomicUsize::new(0);
            let cpu = move || {
                if samples.fetch_add(1, Ordering::Relaxed) == fail_at {
                    Err(failed("injected CPU failure"))
                } else {
                    Ok(0)
                }
            };
            assert!(
                run_with_hooks(
                    config(2, 1, 4),
                    trace(8, 1, 8, 0, true, 50, 0).unwrap(),
                    arrangement,
                    &AtomicBool::new(false),
                    &Hooks {
                        prepare: &prepare,
                        cpu: &cpu
                    }
                )
                .is_err()
            );
        }
        let error = run_with_hooks(
            config(2, 1, 4),
            trace(8, 1, 8, 0, true, 100, 0).unwrap(),
            arrangement,
            &AtomicBool::new(false),
            &Hooks {
                prepare: &|_, _, _| Ok(false),
                cpu: &|| Ok(0),
            },
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cancelled effect in drained run")
        );
    }
}

#[test]
fn commit_and_state_error_edges_are_explicit() {
    let start = Instant::now();
    let cancel = AtomicBool::new(false);
    let abort = AtomicBool::new(false);
    let session = Session {
        start,
        deadline: start + Duration::from_secs(1),
        cancel: &cancel,
        abort: &abort,
    };
    let mut job = Job {
        request: Request {
            id: 0,
            key: 0,
            at_ns: 0,
            work: 0,
            operation: Operation::Add { value: u64::MAX },
        },
        sequence: 0,
        offered_ns: 0,
        admitted_ns: 0,
    };
    let mut state = State::Owned(vec![(0, Slot::default())]);
    assert_eq!(
        state.commit(&job, true, 2, &session).unwrap().0,
        Outcome::Completed { value: u64::MAX }
    );
    assert!(state.commit(&job, true, 2, &session).is_err());
    job.sequence = 1;
    job.request.operation = Operation::Add { value: 1 };
    assert_eq!(
        state.commit(&job, true, 2, &session).unwrap().0,
        Outcome::Completed { value: 0 }
    );
    job.sequence = 2;
    job.request.operation = Operation::Lookup;
    assert_eq!(
        state.commit(&job, true, 2, &session).unwrap().0,
        Outcome::Completed { value: 0 }
    );
    job.request.key = 1;
    assert!(state.commit(&job, true, 2, &session).is_err());
    job.request.key = 2;
    assert!(state.commit(&job, true, 2, &session).is_err());
    let shared = Arc::new(Shared {
        slots: Mutex::new(vec![Slot::default()]),
        changed: Condvar::new(),
    });
    let mut state = State::Shared(Arc::clone(&shared));
    job.request.key = 0;
    abort.store(true, Ordering::Release);
    assert!(
        state
            .commit(&job, true, 2, &session)
            .unwrap_err()
            .to_string()
            .contains("order wait")
    );
    job.sequence = 0;
    assert!(
        state
            .commit(&job, true, 2, &session)
            .unwrap_err()
            .to_string()
            .contains("before commit")
    );
    let _ = std::panic::catch_unwind(|| {
        let _lock = shared.slots.lock().unwrap();
        panic!("poisoned fixture");
    });
    assert!(
        state
            .commit(&job, true, 2, &session)
            .unwrap_err()
            .to_string()
            .contains("poisoned")
    );
}
