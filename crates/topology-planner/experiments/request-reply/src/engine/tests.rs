// Copyright (c) Mike Grier.
use super::*;

#[test]
fn shared_and_assigned_are_distinct_real_queue_layouts() {
    let config = Config {
        workers: 2,
        capacity: 4,
        timeout_ms: 1000,
        cancel_after_admitted: None,
    };
    let shared = shared_service(&config);
    assert_eq!(shared.senders.len(), 1);
    assert!(shared.receivers[0].same_channel(&shared.receivers[1]));
    let assigned = assigned_service(&config);
    assert_eq!(assigned.senders.len(), 2);
    assert!(!assigned.receivers[0].same_channel(&assigned.receivers[1]));
    let request = crate::trace(1, 2, 1, 0, false).unwrap().remove(0);
    assigned.senders[0]
        .send(Job {
            request: request.clone(),
            offered_ns: 0,
            admitted_ns: 0,
        })
        .unwrap();
    assert!(assigned.receivers[1].try_recv().is_err());
    assert_eq!(assigned.receivers[0].recv().unwrap().request.id, request.id);
    shared.senders[0]
        .send(Job {
            request,
            offered_ns: 0,
            admitted_ns: 0,
        })
        .unwrap();
    assert!(shared.receivers[1].try_recv().is_ok());
}

#[test]
fn compute_has_cancellation_and_reference_outcomes() {
    let cancel = AtomicBool::new(false);
    let mut request = crate::trace(1, 1, 1, 0, false).unwrap().remove(0);
    for work in [0, 1, 2, 255, 256, 257, 1000, 1024, 9999, 10000] {
        request.work = work;
        assert_eq!(
            service(&request, &cancel, Instant::now() + Duration::from_secs(1)),
            Outcome::Completed {
                value: crate::reference(&request)
            }
        );
    }
    assert_eq!(
        service(&request, &cancel, Instant::now()),
        Outcome::Cancelled
    );
    cancel.store(true, Ordering::Release);
    assert_eq!(
        service(&request, &cancel, Instant::now() + Duration::from_secs(1)),
        Outcome::Cancelled
    );
}

#[test]
fn prefilled_pressure_and_cancelled_rundown_are_exact() {
    let config = Config {
        workers: 2,
        capacity: 4,
        timeout_ms: 1000,
        cancel_after_admitted: None,
    };
    for arrangement in [Arrangement::Shared, Arrangement::Assigned] {
        let queues = match arrangement {
            Arrangement::Shared => shared_service(&config),
            Arrangement::Assigned => assigned_service(&config),
        };
        for sender in &queues.senders {
            for request in crate::trace(sender.capacity().unwrap(), 2, 4, 0, false).unwrap() {
                assert!(
                    sender
                        .try_send(Job {
                            request,
                            offered_ns: 0,
                            admitted_ns: 0
                        })
                        .is_ok()
                );
            }
            let request = crate::trace(1, 2, 1, 0, false).unwrap().remove(0);
            assert!(matches!(
                sender.try_send(Job {
                    request,
                    offered_ns: 0,
                    admitted_ns: 0
                }),
                Err(TrySendError::Full(_))
            ));
        }
        let (send, receive) = bounded(config.capacity);
        drop(queues.senders);
        let cancel = AtomicBool::new(true);
        let start = Instant::now();
        let mut handled = 0;
        for (index, requests) in queues.receivers.into_iter().enumerate() {
            handled += worker(
                index,
                requests,
                send.clone(),
                start,
                Duration::from_secs(1),
                &cancel,
                &service,
            )
            .unwrap()
            .handled;
        }
        assert_eq!(handled, config.capacity);
        assert_eq!(receive.len(), config.capacity);
        assert!(
            receive
                .try_iter()
                .all(|reply| reply.outcome == Outcome::Cancelled)
        );
    }
}

#[test]
fn worker_panics_and_disconnections_do_not_strand_peers() {
    for arrangement in [Arrangement::Shared, Arrangement::Assigned] {
        let config = Config {
            workers: 2,
            capacity: 4,
            timeout_ms: 1000,
            cancel_after_admitted: None,
        };
        let requests = crate::trace(64, 2, 64, 0, true).unwrap();
        let error = run_with_processor(
            config,
            requests,
            arrangement,
            &AtomicBool::new(false),
            &|request, cancel, deadline| {
                if request.id == 7 {
                    panic!("injected request failure");
                }
                service(request, cancel, deadline)
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("worker"));
    }
    let (send, requests) = bounded(1);
    send.send(Job {
        request: crate::trace(1, 1, 1, 0, false).unwrap().remove(0),
        offered_ns: 0,
        admitted_ns: 0,
    })
    .unwrap();
    drop(send);
    let (reply_send, replies) = bounded(1);
    drop(replies);
    assert!(
        worker(
            0,
            requests,
            reply_send,
            Instant::now(),
            Duration::from_secs(1),
            &AtomicBool::new(false),
            &service
        )
        .unwrap_err()
        .to_string()
        .contains("collector disconnected")
    );
}

#[test]
fn workload_imbalance_is_not_an_ownership_requirement_for_shared_service() {
    for arrangement in [Arrangement::Shared, Arrangement::Assigned] {
        let requests: Vec<_> = crate::trace(32, 2, 32, 0, false)
            .unwrap()
            .into_iter()
            .map(|mut request| {
                request.lane = 0;
                request
            })
            .collect();
        let report = run(
            Config {
                workers: 2,
                capacity: 4,
                timeout_ms: 1000,
                cancel_after_admitted: None,
            },
            requests,
            arrangement,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.replies.len(), 32);
        if arrangement == Arrangement::Assigned {
            assert_eq!(report.workers[0].handled, 32);
            assert_eq!(report.workers[1].handled, 0);
        }
    }
}

#[test]
fn runner_verifies_results_before_returning_success() {
    for arrangement in [Arrangement::Shared, Arrangement::Assigned] {
        let config = Config {
            workers: 2,
            capacity: 4,
            timeout_ms: 1000,
            cancel_after_admitted: None,
        };
        let error = run_with_processor(
            config,
            crate::trace(8, 2, 8, 0, false).unwrap(),
            arrangement,
            &AtomicBool::new(false),
            &|request, _, _| Outcome::Completed {
                value: crate::reference(request).wrapping_add(1),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("incorrect result"));
    }
}

#[test]
fn deadline_during_service_is_a_terminal_outcome_not_a_verification_error() {
    for arrangement in [Arrangement::Shared, Arrangement::Assigned] {
        let config = Config {
            workers: 2,
            capacity: 4,
            timeout_ms: 5,
            cancel_after_admitted: None,
        };
        let report = run_with_processor(
            config,
            crate::trace(16, 2, 16, 0, false).unwrap(),
            arrangement,
            &AtomicBool::new(false),
            &|request, cancel, deadline| {
                while Instant::now() < deadline && !cancel.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                service(request, cancel, deadline)
            },
        )
        .unwrap();
        assert_eq!(report.stop, StopReason::Deadline);
        assert_eq!(report.replies.len() + report.unadmitted.len(), 16);
        assert!(
            report
                .replies
                .iter()
                .all(|reply| reply.outcome == Outcome::Cancelled)
        );
        verify(&report).unwrap();
    }
}
