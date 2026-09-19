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

fn fixture() -> Report {
    let requests = vec![
        Request {
            id: 90,
            key: 0,
            at_ns: 0,
            work: 0,
            operation: Operation::Add { value: 7 },
        },
        Request {
            id: 12,
            key: 1,
            at_ns: 0,
            work: 0,
            operation: Operation::Add { value: 9 },
        },
        Request {
            id: 2,
            key: 0,
            at_ns: 0,
            work: 0,
            operation: Operation::Lookup,
        },
    ];
    let replies = vec![
        Reply {
            id: 90,
            key: 0,
            sequence: 0,
            worker: 0,
            scheduled_ns: 0,
            offered_ns: 1,
            admitted_ns: 2,
            started_ns: 5,
            committed_ns: 10,
            finished_ns: 10,
            collected_ns: 20,
            outcome: Outcome::Completed { value: 7 },
        },
        Reply {
            id: 12,
            key: 1,
            sequence: 0,
            worker: 1,
            scheduled_ns: 0,
            offered_ns: 2,
            admitted_ns: 3,
            started_ns: 5,
            committed_ns: 8,
            finished_ns: 8,
            collected_ns: 15,
            outcome: Outcome::Completed { value: 9 },
        },
        Reply {
            id: 2,
            key: 0,
            sequence: 1,
            worker: 0,
            scheduled_ns: 0,
            offered_ns: 3,
            admitted_ns: 4,
            started_ns: 11,
            committed_ns: 12,
            finished_ns: 12,
            collected_ns: 21,
            outcome: Outcome::Completed { value: 7 },
        },
    ];
    Report {
        schema: "stateful-v1".into(),
        evidence_class: "synthetic_verifier_fixture".into(),
        placement: "logical_only".into(),
        config: config(2, 2, 4),
        arrangement: Arrangement::KeyOwned,
        stop: StopReason::Drained,
        wall_ns: 30,
        request_slots: 4,
        reply_slots: 4,
        state_entries: 2,
        peak_outstanding: 3,
        key_peaks: vec![2, 1],
        lane_peaks: vec![2, 1],
        credit_full_observations: 0,
        lane_full_observations: vec![0, 0],
        trace: requests,
        replies,
        unadmitted: vec![],
        credit_events: vec![
            CreditEvent::Admitted(90),
            CreditEvent::Admitted(12),
            CreditEvent::Admitted(2),
            CreditEvent::Collected(12),
            CreditEvent::Collected(90),
            CreditEvent::Collected(2),
        ],
        workers: vec![
            WorkerReport {
                worker: 0,
                completed: 2,
                cancelled: 0,
                active_ns: 6,
                cpu_ns: 0,
            },
            WorkerReport {
                worker: 1,
                completed: 1,
                cancelled: 0,
                active_ns: 3,
                cpu_ns: 0,
            },
        ],
        final_state: vec![7, 9],
    }
}

#[test]
fn serial_oracle_checks_intermediate_values_order_and_state() {
    let valid = fixture();
    verify(&valid).unwrap();
    for defect in 0..14 {
        let mut report = valid.clone();
        match defect {
            0 => report.replies[2].outcome = Outcome::Completed { value: 0 },
            1 => report.final_state[0] = 0,
            2 => report.replies[2].sequence = 0,
            3 => report.replies[2].committed_ns = 9,
            4 => report.replies[2].worker = 1,
            5 => report.replies[2].id = 90,
            6 => report.replies[2].key = 1,
            7 => report.peak_outstanding = 2,
            8 => report.key_peaks[0] = 3,
            9 => report.credit_events.push(CreditEvent::Collected(2)),
            10 => report.replies[0].outcome = Outcome::Cancelled,
            11 => report.replies[0].scheduled_ns = 1,
            12 => report.workers[0].completed = 1,
            _ => report.unadmitted.push(90),
        }
        assert!(verify(&report).is_err(), "defect {defect}");
    }
    let mut reordered = valid.clone();
    reordered.replies.reverse();
    verify(&reordered).unwrap();
    let aggregate = summary(&valid, None, None).unwrap();
    assert_eq!(
        (
            aggregate.completed.count,
            aggregate.completed.p50_ns,
            aggregate.completed.max_ns
        ),
        (3, 10, 12)
    );
    assert_eq!(summary(&valid, Some(0), None).unwrap().completed.count, 2);
    assert_eq!(summary(&valid, None, Some(1)).unwrap().completed.count, 1);
    assert!(summary(&valid, Some(2), None).is_err());
}

#[test]
fn deterministic_key_mix_bounds_and_trace_order() {
    for count in [0, 1, 2, 3, 7, 8, 17, 32, 65, 100] {
        for mix in [0, 50, 100] {
            let requests = trace(count, 7, 4, 1000, true, mix, 100).unwrap();
            validate(&config(3, 7, 6), &requests).unwrap();
            assert_eq!(requests, trace(count, 7, 4, 1000, true, mix, 100).unwrap());
            if mix == 0 {
                assert!(
                    requests
                        .iter()
                        .all(|request| request.operation == Operation::Lookup)
                );
            }
            if mix == 100 {
                assert!(
                    requests
                        .iter()
                        .all(|request| matches!(request.operation, Operation::Add { .. }))
                );
            }
        }
    }
    for keys in [0, MAX_KEYS + 1, usize::MAX] {
        assert!(validate(&config(2, keys, 4), &[]).is_err());
        assert!(trace(1, keys, 1, 0, false, 50, 1).is_err());
    }
    let mut bad = trace(2, 2, 1, 1, false, 50, 1).unwrap();
    bad[0].key = 2;
    assert!(validate(&config(2, 2, 4), &bad).is_err());
    assert!(trace(1, 1, 1, 0, false, 101, 1).is_err());
    assert!(trace(1, 1, 1, 0, false, 50, crate::MAX_WORK + 1).is_err());
}
