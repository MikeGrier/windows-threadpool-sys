// Copyright (c) Mike Grier.
use super::*;

fn config(workers: usize, capacity: usize) -> Config {
    Config {
        workers,
        capacity,
        timeout_ms: 1000,
        cancel_after_admitted: None,
    }
}

#[test]
fn steady_burst_skew_and_empty_traces_are_deterministic() {
    for count in [0, 1, 2, 3, 7, 8, 9, 31, 100, 512] {
        for burst in [1, 4, 32] {
            let requests = trace(count, 4, burst, 1000, true).unwrap();
            assert_eq!(requests, trace(count, 4, burst, 1000, true).unwrap());
            validate(&config(4, 8), &requests).unwrap();
            for (index, request) in requests.iter().enumerate() {
                assert_eq!(request.at_ns, (index / burst) as u64 * 1000);
                assert_eq!(
                    request.lane,
                    if index % 8 == 0 { (index / 8) % 4 } else { 0 }
                );
            }
        }
    }
}

#[test]
fn invalid_limits_and_trace_inputs_are_rejected() {
    for (workers, capacity) in [(0, 2), (17, 34), (2, 0), (4, 2), (2, 3), (2, 8192)] {
        assert!(validate(&config(workers, capacity), &[]).is_err());
    }
    for timeout in [0, 5001, u64::MAX] {
        let mut candidate = config(2, 4);
        candidate.timeout_ms = timeout;
        assert!(validate(&candidate, &[]).is_err());
    }
    let base = trace(3, 2, 1, 1, false).unwrap();
    for defect in 0..6 {
        let mut requests = base.clone();
        match defect {
            0 => requests[1].id = requests[0].id,
            1 => requests[0].lane = 2,
            2 => requests[2].at_ns = 0,
            3 => requests[2].at_ns = MAX_SPAN_NS + 1,
            4 => requests[0].work = MAX_WORK + 1,
            _ => requests = vec![base[0].clone(); MAX_REQUESTS + 1],
        }
        assert!(validate(&config(2, 4), &requests).is_err());
    }
    let mut candidate = config(2, 4);
    candidate.cancel_after_admitted = Some(1);
    assert!(validate(&candidate, &[]).is_err());
    for (count, workers, burst, interval) in [
        (1, 0, 1, 1),
        (1, 17, 1, 1),
        (1, 1, 0, 1),
        (MAX_REQUESTS + 1, 1, 1, 1),
        (3, 1, 1, u64::MAX),
    ] {
        assert!(trace(count, workers, burst, interval, false).is_err());
    }
}

#[test]
fn reference_has_independent_known_results_and_wraps() {
    let mut request = Request {
        id: 0,
        lane: 0,
        at_ns: 0,
        value: 10,
        work: 4,
    };
    assert_eq!(reference(&request), 20);
    request.work = 0;
    assert_eq!(reference(&request), 10);
    request.value = u64::MAX;
    request.work = 1;
    assert_eq!(reference(&request), 0);
}

#[test]
fn both_candidates_preserve_ten_shapes_correlation_and_budgets() {
    for (count, workers, capacity, burst, skew) in [
        (0, 1, 1, 1, false),
        (1, 1, 1, 1, false),
        (2, 2, 2, 2, false),
        (7, 2, 4, 3, true),
        (17, 4, 4, 17, true),
        (32, 2, 8, 4, false),
        (33, 3, 6, 8, true),
        (64, 4, 16, 1, false),
        (127, 2, 4, 127, true),
        (128, 8, 16, 32, false),
    ] {
        let requests = trace(count, workers, burst, 100, skew).unwrap();
        for arrangement in [Arrangement::Shared, Arrangement::Assigned] {
            let report = run(
                config(workers, capacity),
                requests.clone(),
                arrangement,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            verify(&report).unwrap();
            assert_eq!(report.stop, StopReason::Drained);
            assert_eq!(report.replies.len(), count);
            assert!(report.unadmitted.is_empty());
            assert!(report.peak_outstanding <= capacity);
            assert!(
                report
                    .replies
                    .iter()
                    .all(|reply| matches!(reply.outcome, Outcome::Completed { .. }))
            );
        }
    }
}

#[test]
fn cancellation_before_and_during_admission_preserves_census() {
    use std::sync::atomic::AtomicBool;
    let requests = trace(32, 2, 32, 0, true).unwrap();
    for arrangement in [Arrangement::Shared, Arrangement::Assigned] {
        for count in [0, 1, 2, 8, 31] {
            let mut candidate = config(2, 8);
            candidate.cancel_after_admitted = Some(count);
            let report = run(
                candidate,
                requests.clone(),
                arrangement,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(report.stop, StopReason::Cancelled);
            assert_eq!(report.replies.len(), count);
            assert_eq!(report.unadmitted.len(), 32 - count);
        }
        let report = run(
            config(2, 8),
            requests.clone(),
            arrangement,
            &AtomicBool::new(true),
        )
        .unwrap();
        assert_eq!(report.replies.len(), 0);
        let mut candidate = config(2, 8);
        candidate.timeout_ms = 1;
        let mut late = requests.clone();
        for request in &mut late {
            request.at_ns = MAX_SPAN_NS;
        }
        let report = run(candidate, late, arrangement, &AtomicBool::new(false)).unwrap();
        assert_eq!(report.stop, StopReason::Deadline);
        assert_eq!(report.unadmitted.len(), 32);
    }
}

fn reordered_report() -> Report {
    let requests = trace(2, 2, 2, 0, false).unwrap();
    let replies: Vec<_> = requests
        .iter()
        .map(|request| Reply {
            id: request.id,
            lane: request.lane,
            worker: request.lane,
            scheduled_ns: 0,
            offered_ns: 5,
            admitted_ns: 10 + request.id,
            started_ns: 15,
            finished_ns: if request.id == 0 { 90 } else { 40 },
            collected_ns: if request.id == 0 { 100 } else { 50 },
            outcome: Outcome::Completed {
                value: reference(request),
            },
        })
        .collect();
    Report {
        schema: "request-reply-v1".into(),
        evidence_class: "synthetic_accounting_test".into(),
        config: config(2, 2),
        arrangement: Arrangement::Assigned,
        stop: StopReason::Drained,
        wall_ns: 110,
        request_slots: 2,
        reply_slots: 2,
        peak_outstanding: 2,
        lane_peaks: vec![1, 1],
        credit_full_observations: 0,
        lane_full_observations: vec![0, 0],
        trace: requests,
        replies,
        unadmitted: Vec::new(),
        credit_events: vec![
            CreditEvent::Admitted(0),
            CreditEvent::Admitted(1),
            CreditEvent::Collected(1),
            CreditEvent::Collected(0),
        ],
        workers: vec![
            WorkerReport {
                worker: 0,
                handled: 1,
                completed: 1,
                cancelled: 0,
                active_ns: 75,
            },
            WorkerReport {
                worker: 1,
                handled: 1,
                completed: 1,
                cancelled: 0,
                active_ns: 25,
            },
        ],
    }
}

#[test]
fn verifier_accepts_reordering_but_rejects_corruption_ownership_and_credit_defects() {
    let valid = reordered_report();
    verify(&valid).unwrap();
    for defect in 0..17 {
        let mut report = valid.clone();
        match defect {
            0 => report.replies[0].id = 1,
            1 => report.replies[0].id = 42,
            2 => report.replies[0].lane = 1,
            3 => report.replies[0].worker = 1,
            4 => report.replies[0].outcome = Outcome::Completed { value: 0 },
            5 => report.replies[0].outcome = Outcome::Cancelled,
            6 => report.replies[0].scheduled_ns = report.replies[0].admitted_ns,
            7 => report.replies[0].offered_ns = report.replies[0].started_ns,
            8 => report.replies[0].started_ns = 9,
            9 => report.replies[0].collected_ns = 1,
            10 => {
                report.credit_events.remove(0);
            }
            11 => report.credit_events.push(CreditEvent::Collected(0)),
            12 => report.peak_outstanding = 1,
            13 => report.lane_peaks[0] = 2,
            14 => report.unadmitted.push(0),
            15 => report.workers[0].handled = 0,
            _ => report.credit_events.swap(0, 1),
        }
        assert!(verify(&report).is_err(), "defect {defect} survived");
    }
    let mut shared = valid.clone();
    shared.arrangement = Arrangement::Shared;
    shared.replies[0].worker = 1;
    shared.workers[0] = WorkerReport {
        worker: 0,
        handled: 0,
        completed: 0,
        cancelled: 0,
        active_ns: 0,
    };
    shared.workers[1] = WorkerReport {
        worker: 1,
        handled: 2,
        completed: 2,
        cancelled: 0,
        active_ns: 100,
    };
    verify(&shared).unwrap();
}

#[test]
fn response_distributions_start_at_scheduled_arrival_and_keep_lane_identity() {
    let report = reordered_report();
    let summary = response_summary(&report, None).unwrap();
    assert_eq!(
        (
            summary.completed.count,
            summary.completed.p50_ns,
            summary.completed.p99_ns
        ),
        (2, 40, 90)
    );
    assert_eq!(summary.offer_lateness.max_ns, 5);
    assert_eq!(summary.admission_wait.max_ns, 6);
    assert_eq!(
        response_summary(&report, Some(0)).unwrap().completed.p50_ns,
        90
    );
    assert_eq!(
        response_summary(&report, Some(1)).unwrap().completed.p50_ns,
        40
    );
    assert!(response_summary(&report, Some(2)).is_err());
}
