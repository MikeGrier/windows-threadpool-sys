// Copyright (c) Mike Grier.
use super::*;

const ALL: [Arrangement; 3] = [
    Arrangement::SerialWhole,
    Arrangement::OwnedJoin,
    Arrangement::ScatteredJoin,
];

fn config(workers: usize, capacity: usize, cancel: Option<usize>) -> Config {
    Config {
        workers,
        capacity,
        timeout_ms: 1000,
        cancel_after_admitted: cancel,
    }
}

fn go(config: Config, records: Vec<Record>, arrangement: Arrangement) -> io::Result<Report> {
    run_with_hooks(
        config,
        records,
        arrangement,
        &AtomicBool::new(false),
        &Hooks {
            service: &service,
            cpu: &|| Ok(0),
        },
    )
}

fn ordered(report: &Report) -> Vec<&Completion> {
    let mut completions: Vec<_> = report.completions.iter().collect();
    completions.sort_by_key(|completion| completion.position);
    completions
}

fn expected_log(records: &[Record]) -> Vec<JoinedRecord> {
    records
        .iter()
        .filter(|record| record.fail_at.is_none())
        .map(|record| JoinedRecord {
            id: record.id,
            value: reference(record),
        })
        .collect()
}

#[test]
fn all_three_arrangements_join_the_same_records_over_ten_shapes() {
    for (count, burst, interval, work, units, broadcast, skew, slow) in [
        (0, 1, 0, 0, 1, 0, 0, 0),
        (1, 1, 0, 500, 1, 0, 0, 0),
        (2, 1, 0, 500, 2, 2, 0, 0),
        (8, 1, 50_000, 400, 4, 3, 2, 0),
        (12, 4, 100_000, 400, 4, 2, 3, 5),
        (16, 16, 0, 0, 8, 4, 2, 0),
        (16, 16, 0, 300, 3, 1, 0, 0),
        (17, 5, 0, 300, 5, 3, 2, 4),
        (24, 8, 0, 200, 16, 2, 3, 7),
        (25, 25, 0, 150, 7, 5, 4, 3),
        (32, 32, 0, 100, 2, 2, 0, 0),
    ] {
        let records = trace(TraceSpec {
            count,
            burst,
            interval_ns: interval,
            work,
            units,
            broadcast_every: broadcast,
            skew_every: skew,
            slow_every: slow,
            ..Default::default()
        })
        .unwrap();
        let expected = expected_log(&records);
        for workers in [1, 2, 4] {
            for arrangement in ALL {
                let report = go(
                    config(workers, workers * 4, None),
                    records.clone(),
                    arrangement,
                )
                .unwrap();
                assert_eq!(report.stop, StopReason::Drained);
                assert_eq!(
                    report.log, expected,
                    "{arrangement:?} with {workers} workers"
                );
                assert_eq!(report.completions.len(), count);
                assert!(report.unadmitted.is_empty());
                assert_eq!(report.residual_partial_units, 0);
                assert_eq!(report.discarded_partial_units, 0);
            }
        }
    }
}

#[test]
fn every_join_consumes_its_declared_unit_set_exactly_once() {
    let records = trace(TraceSpec {
        count: 24,
        burst: 8,
        work: 200,
        units: 6,
        broadcast_every: 3,
        skew_every: 2,
        slow_every: 5,
        ..Default::default()
    })
    .unwrap();
    for arrangement in ALL {
        let report = go(config(3, 6, None), records.clone(), arrangement).unwrap();
        for (record, completion) in records.iter().zip(ordered(&report)) {
            let mut indices: Vec<_> = completion.units.iter().map(|unit| unit.index).collect();
            indices.sort_unstable();
            assert_eq!(
                indices,
                (0..record.units()).collect::<Vec<_>>(),
                "membership for {:?}",
                record.shape
            );
        }
    }
}

#[test]
fn scatter_and_broadcast_are_distinct_shapes_at_every_unit_count() {
    for count in 1..=16 {
        let value = 0x0123_4567_89AB_CDEF_u64;
        let scatter = Record {
            id: 0,
            at_ns: 0,
            value,
            shape: Shape::Scatter { parts: count },
            work: 0,
            slow_unit: None,
            fail_at: None,
        };
        let broadcast = Record {
            shape: Shape::Broadcast { branches: count },
            ..scatter.clone()
        };
        assert_ne!(
            reference(&scatter),
            reference(&broadcast),
            "shapes must not collapse at {count} units"
        );
        for index in 0..count {
            assert_ne!(unit_result(&scatter, index), unit_result(&broadcast, index));
        }
    }
}

#[test]
fn owned_join_routes_every_unit_of_a_record_to_its_owner() {
    let records = trace(TraceSpec {
        count: 32,
        burst: 8,
        work: 200,
        units: 5,
        broadcast_every: 3,
        skew_every: 2,
        ..Default::default()
    })
    .unwrap();
    let report = go(config(3, 6, None), records.clone(), Arrangement::OwnedJoin).unwrap();
    for (record, completion) in records.iter().zip(ordered(&report)) {
        let expected = owner(record.id, 3);
        assert_eq!(completion.joiner, expected);
        assert!(completion.units.iter().all(|unit| unit.worker == expected));
    }
}

#[test]
fn only_the_scattered_join_holds_partial_state_across_a_thread() {
    let records = trace(TraceSpec {
        count: 32,
        burst: 32,
        work: 300,
        units: 8,
        broadcast_every: 3,
        skew_every: 2,
        ..Default::default()
    })
    .unwrap();
    for arrangement in ALL {
        let report = go(config(3, 6, None), records.clone(), arrangement).unwrap();
        assert_eq!(report.residual_partial_units, 0);
        if arrangement == Arrangement::ScatteredJoin {
            assert!(report.peak_partial_records >= 1);
            assert!(report.peak_partial_units >= 1);
            assert!(report.peak_partial_records <= report.config.capacity);
            assert_eq!(report.record_slots, 0);
            assert_eq!(report.unit_slots, 6 * 16);
        } else {
            assert_eq!(report.peak_partial_records, 0);
            assert_eq!(report.peak_partial_units, 0);
            assert_eq!(report.record_slots, 6);
            assert_eq!(report.unit_slots, 0);
        }
    }
}

#[test]
fn an_injected_unit_failure_aborts_its_record_and_discards_a_complete_unit_set() {
    for fail_every in [1, 2, 3, 5] {
        let records = trace(TraceSpec {
            count: 12,
            burst: 4,
            work: 200,
            units: 4,
            broadcast_every: 3,
            fail_every,
            ..Default::default()
        })
        .unwrap();
        let expected = expected_log(&records);
        let discarded: usize = records
            .iter()
            .filter(|record| record.fail_at.is_some())
            .map(|record| record.units() as usize)
            .sum();
        for arrangement in ALL {
            let report = go(config(2, 4, None), records.clone(), arrangement).unwrap();
            assert_eq!(report.log, expected);
            assert_eq!(report.discarded_partial_units, discarded);
            assert_eq!(report.residual_partial_units, 0);
            for (record, completion) in records.iter().zip(ordered(&report)) {
                match (record.fail_at, completion.outcome) {
                    (Some(injected), Outcome::Aborted { unit }) => assert_eq!(injected, unit),
                    (None, Outcome::Joined { value }) => assert_eq!(value, reference(record)),
                    pair => panic!("unexpected outcome pairing {pair:?}"),
                }
            }
        }
    }
}

#[test]
fn cancellation_resolves_every_admitted_record_and_reclaims_its_branches() {
    let records = trace(TraceSpec {
        count: 64,
        burst: 64,
        work: 20_000,
        units: 6,
        broadcast_every: 3,
        skew_every: 2,
        slow_every: 4,
        ..Default::default()
    })
    .unwrap();
    for arrangement in ALL {
        let report = go(config(2, 4, Some(4)), records.clone(), arrangement).unwrap();
        assert_eq!(report.stop, StopReason::Cancelled);
        assert_eq!(report.residual_partial_units, 0);
        let sequence = ordered(&report);
        if let Some(first) = sequence
            .iter()
            .position(|completion| completion.outcome == Outcome::Cancelled)
        {
            assert!(
                sequence[first..]
                    .iter()
                    .all(|completion| completion.outcome == Outcome::Cancelled)
            );
        }
        assert_eq!(
            report.completions.len() + report.unadmitted.len(),
            records.len()
        );
        let discarded: usize = sequence
            .iter()
            .filter(|completion| !matches!(completion.outcome, Outcome::Joined { .. }))
            .map(|completion| completion.units.len())
            .sum();
        assert_eq!(report.discarded_partial_units, discarded);
    }
}

#[test]
fn cancellation_overrides_an_injected_failure_it_reaches_first() {
    let records = trace(TraceSpec {
        count: 128,
        burst: 128,
        work: 400_000,
        units: 4,
        broadcast_every: 3,
        fail_every: 2,
        ..Default::default()
    })
    .unwrap();
    assert!(records[1].fail_at.is_some());
    for arrangement in ALL {
        let report = go(config(2, 4, Some(4)), records.clone(), arrangement).unwrap();
        assert_eq!(report.stop, StopReason::Cancelled);
        let sequence = ordered(&report);
        let first = sequence
            .iter()
            .position(|completion| completion.outcome == Outcome::Cancelled)
            .expect("a cancelled run must resolve at least one record as cancelled");
        assert!(
            sequence[first..]
                .iter()
                .all(|completion| completion.outcome == Outcome::Cancelled),
            "an injected failure reached after cancellation must not report an abort"
        );
    }
}

#[test]
fn external_cancellation_before_the_first_arrival_admits_nothing() {
    let records = trace(TraceSpec {
        count: 16,
        burst: 16,
        work: 1000,
        units: 4,
        broadcast_every: 2,
        ..Default::default()
    })
    .unwrap();
    for arrangement in ALL {
        let report = run_with_hooks(
            config(2, 4, None),
            records.clone(),
            arrangement,
            &AtomicBool::new(true),
            &Hooks {
                service: &service,
                cpu: &|| Ok(0),
            },
        )
        .unwrap();
        assert_eq!(report.stop, StopReason::Cancelled);
        assert!(report.log.is_empty());
        assert!(report.completions.is_empty());
        assert_eq!(report.unadmitted.len(), records.len());
    }
}

#[test]
fn a_controlled_cancellation_without_a_stop_condition_is_rejected_by_the_runner() {
    let records = trace(TraceSpec {
        count: 8,
        burst: 8,
        work: 100,
        units: 3,
        broadcast_every: 2,
        ..Default::default()
    })
    .unwrap();
    let interrupt = |record: &Record, index: u32, _: &AtomicBool, _: Instant| {
        Ok(!(record.id >= 4 && index == 0))
    };
    for arrangement in ALL {
        assert!(
            run_with_hooks(
                config(1, 2, None),
                records.clone(),
                arrangement,
                &AtomicBool::new(false),
                &Hooks {
                    service: &interrupt,
                    cpu: &|| Ok(0),
                },
            )
            .is_err(),
            "a drained run reporting cancelled outcomes must fail through the real runner"
        );
    }
}

#[test]
fn a_failing_service_or_clock_joins_every_worker_and_propagates() {
    let records = trace(TraceSpec {
        count: 12,
        burst: 12,
        work: 100,
        units: 3,
        broadcast_every: 2,
        ..Default::default()
    })
    .unwrap();
    for arrangement in ALL {
        let broken =
            |_: &Record, _: u32, _: &AtomicBool, _: Instant| Err(failed("unit service failed"));
        assert!(
            run_with_hooks(
                config(2, 4, None),
                records.clone(),
                arrangement,
                &AtomicBool::new(false),
                &Hooks {
                    service: &broken,
                    cpu: &|| Ok(0),
                },
            )
            .is_err()
        );
        assert!(
            run_with_hooks(
                config(2, 4, None),
                records.clone(),
                arrangement,
                &AtomicBool::new(false),
                &Hooks {
                    service: &service,
                    cpu: &|| Err(failed("clock failed")),
                },
            )
            .is_err()
        );
    }
}

#[test]
fn invalid_fan_out_shapes_are_rejected_before_any_thread_starts() {
    let base = trace(TraceSpec {
        count: 4,
        work: 100,
        units: 4,
        ..Default::default()
    })
    .unwrap();
    let broken = |mutate: fn(&mut Record)| {
        let mut records = base.clone();
        mutate(&mut records[1]);
        records
    };
    for records in [
        broken(|record| record.shape = Shape::Scatter { parts: 0 }),
        broken(|record| record.shape = Shape::Broadcast { branches: 0 }),
        broken(|record| {
            record.shape = Shape::Scatter {
                parts: MAX_UNITS + 1,
            }
        }),
        broken(|record| record.slow_unit = Some(record.units())),
        broken(|record| record.fail_at = Some(record.units())),
        broken(|record| record.work = crate::MAX_WORK + 1),
    ] {
        assert!(
            go(config(2, 4, None), records, Arrangement::ScatteredJoin).is_err(),
            "an invalid arrangement must be refused before it is measured"
        );
    }
    for settings in [
        config(0, 4, None),
        config(3, 4, None),
        config(2, 1, None),
        config(2, 4, Some(99)),
    ] {
        assert!(go(settings, base.clone(), Arrangement::OwnedJoin).is_err());
    }
}

#[test]
fn pressure_keeps_admission_inside_the_credit_ceiling() {
    let records = trace(TraceSpec {
        count: 64,
        burst: 64,
        work: 2000,
        units: 8,
        broadcast_every: 3,
        skew_every: 2,
        slow_every: 5,
        ..Default::default()
    })
    .unwrap();
    for arrangement in ALL {
        let report = go(config(2, 2, None), records.clone(), arrangement).unwrap();
        assert_eq!(report.log, expected_log(&records));
        assert!(report.peak_outstanding <= 2);
    }
}
