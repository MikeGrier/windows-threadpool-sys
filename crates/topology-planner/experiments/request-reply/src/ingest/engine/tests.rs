// Copyright (c) Mike Grier.
use super::*;

fn config(workers: usize, capacity: usize, reorder: usize, cancel: Option<usize>) -> Config {
    Config {
        limits: Limits {
            workers,
            capacity,
            timeout_ms: 1000,
            cancel_after_admitted: cancel,
        },
        reorder_capacity: reorder,
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

fn expected_log(records: &[Record]) -> Vec<Published> {
    records
        .iter()
        .filter(|record| record.fail_at.is_none())
        .map(|record| Published {
            id: record.id,
            value: reference(record),
        })
        .collect()
}

#[test]
fn both_arrangements_publish_the_same_records_in_declared_order_over_ten_shapes() {
    for (count, burst, interval, transform, publish, slow) in [
        (0, 1, 0, 0, 0, false),
        (1, 1, 0, 1000, 500, false),
        (2, 1, 0, 1000, 0, false),
        (8, 1, 50_000, 1000, 200, false),
        (16, 4, 200_000, 1000, 200, false),
        (16, 16, 0, 0, 0, false),
        (16, 16, 0, 4000, 0, true),
        (16, 16, 0, 0, 4000, false),
        (24, 8, 0, 800, 800, false),
        (25, 6, 0, 1200, 300, true),
        (32, 32, 0, 500, 500, false),
    ] {
        let records = trace(count, burst, interval, transform, publish, slow, 0).unwrap();
        let expected = expected_log(&records);
        for workers in [1, 2, 3] {
            let settings = config(workers, workers * 4, workers * 2, None);
            let serial = go(settings.clone(), records.clone(), Arrangement::SerialOwner).unwrap();
            let staged = go(settings, records.clone(), Arrangement::StagedPipeline).unwrap();
            for report in [&serial, &staged] {
                assert_eq!(report.stop, StopReason::Drained);
                assert_eq!(report.log, expected);
                assert_eq!(report.completions.len(), count);
                assert!(report.unadmitted.is_empty());
                assert!(report.peak_outstanding <= report.config.limits.capacity);
            }
            assert_eq!(serial.workers.len(), 1);
            assert_eq!(staged.workers.len(), workers + 1);
        }
    }
}

#[test]
fn publication_advances_in_position_order_even_when_transforms_overlap() {
    let records = trace(24, 24, 0, 2000, 0, true, 0).unwrap();
    let report = go(
        config(4, 8, 4, None),
        records.clone(),
        Arrangement::StagedPipeline,
    )
    .unwrap();
    assert_eq!(report.log, expected_log(&records));
    let mut previous = 0;
    for completion in ordered(&report) {
        assert!(
            completion.published_ns >= previous,
            "publication must not move backwards in declared order"
        );
        previous = completion.published_ns;
    }
    assert!(report.peak_reorder_occupancy >= 1);
    assert!(report.peak_reorder_occupancy <= report.reorder_slots);
}

#[test]
fn a_single_slot_reorder_window_still_drains_every_record() {
    let records = trace(32, 32, 0, 300, 0, false, 0).unwrap();
    let report = go(
        config(4, 8, 1, None),
        records.clone(),
        Arrangement::StagedPipeline,
    )
    .unwrap();
    assert_eq!(report.log, expected_log(&records));
    assert_eq!(report.reorder_slots, 1);
    assert_eq!(report.peak_reorder_occupancy, 1);
}

#[test]
fn the_serial_owner_reports_no_reorder_buffer_at_all() {
    let records = trace(16, 16, 0, 500, 500, false, 0).unwrap();
    let report = go(config(4, 8, 4, None), records, Arrangement::SerialOwner).unwrap();
    assert_eq!(report.reorder_slots, 0);
    assert_eq!(report.peak_reorder_occupancy, 0);
    assert_eq!(report.reorder_full_observations, 0);
    assert_eq!(report.workers[0].role, Role::Owner);
}

#[test]
fn an_injected_failure_aborts_its_own_record_and_still_advances_publication() {
    for fail_every in [1, 2, 3, 5] {
        let records = trace(12, 4, 0, 500, 100, false, fail_every).unwrap();
        let expected = expected_log(&records);
        for arrangement in [Arrangement::SerialOwner, Arrangement::StagedPipeline] {
            let report = go(config(2, 8, 4, None), records.clone(), arrangement).unwrap();
            assert_eq!(report.log, expected);
            assert_eq!(report.completions.len(), records.len());
            for (record, completion) in records.iter().zip(ordered(&report)) {
                match (record.fail_at, completion.outcome) {
                    (Some(injected), Outcome::Aborted { stage }) => assert_eq!(injected, stage),
                    (None, Outcome::Published { value }) => assert_eq!(value, reference(record)),
                    pair => panic!("unexpected outcome pairing {pair:?}"),
                }
            }
        }
    }
}

#[test]
fn cancellation_resolves_every_admitted_record_and_leaves_a_cancelled_suffix() {
    for arrangement in [Arrangement::SerialOwner, Arrangement::StagedPipeline] {
        let records = trace(64, 64, 0, 20_000, 0, false, 0).unwrap();
        let report = go(config(2, 8, 4, Some(8)), records.clone(), arrangement).unwrap();
        assert_eq!(report.stop, StopReason::Cancelled);
        let sequence = ordered(&report);
        if let Some(first) = sequence
            .iter()
            .position(|completion| completion.outcome == Outcome::Cancelled)
        {
            assert!(
                sequence[first..]
                    .iter()
                    .all(|completion| completion.outcome == Outcome::Cancelled),
                "a cancelled suffix must never resume publishing"
            );
        }
        assert_eq!(
            report.log.len(),
            sequence
                .iter()
                .filter(|completion| matches!(completion.outcome, Outcome::Published { .. }))
                .count()
        );
        assert_eq!(
            report.completions.len() + report.unadmitted.len(),
            records.len()
        );
    }
}

#[test]
fn cancellation_overrides_an_injected_failure_it_reaches_first() {
    // Record 0 carries the full transform budget and record 1 an injected transform
    // failure, so cancellation lands between them and the abort must become a
    // cancellation rather than an abort after the stop.
    let records = trace(128, 128, 0, 400_000, 0, false, 2).unwrap();
    assert_eq!(records[1].fail_at, Some(Stage::Transform));
    for arrangement in [Arrangement::SerialOwner, Arrangement::StagedPipeline] {
        let report = go(config(2, 8, 4, Some(4)), records.clone(), arrangement).unwrap();
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
    let records = trace(16, 16, 0, 1000, 0, false, 0).unwrap();
    let report = run_with_hooks(
        config(2, 8, 4, None),
        records.clone(),
        Arrangement::StagedPipeline,
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

#[test]
fn a_controlled_cancellation_without_a_stop_condition_is_rejected_by_the_runner() {
    let records = trace(8, 8, 0, 100, 0, false, 0).unwrap();
    let interrupt = |record: &Record, stage: Stage, _: &AtomicBool, _: Instant| {
        Ok(!(stage == Stage::Transform && record.id >= 4))
    };
    let result = run_with_hooks(
        config(1, 4, 2, None),
        records,
        Arrangement::SerialOwner,
        &AtomicBool::new(false),
        &Hooks {
            service: &interrupt,
            cpu: &|| Ok(0),
        },
    );
    assert!(
        result.is_err(),
        "a drained run reporting cancelled outcomes must fail through the real runner"
    );
}

#[test]
fn a_failing_service_or_clock_joins_every_worker_and_propagates() {
    let records = trace(12, 12, 0, 100, 100, false, 0).unwrap();
    for arrangement in [Arrangement::SerialOwner, Arrangement::StagedPipeline] {
        let broken =
            |_: &Record, _: Stage, _: &AtomicBool, _: Instant| Err(failed("service failed"));
        assert!(
            run_with_hooks(
                config(2, 8, 4, None),
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
                config(2, 8, 4, None),
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
fn invalid_configurations_and_records_are_rejected_before_any_thread_starts() {
    let records = trace(4, 1, 0, 100, 100, false, 0).unwrap();
    for settings in [
        config(2, 8, 0, None),
        config(2, 8, 9, None),
        config(0, 8, 4, None),
        config(3, 8, 4, None),
        config(2, 1, 1, None),
        Config {
            limits: Limits {
                workers: 2,
                capacity: 8,
                timeout_ms: 0,
                cancel_after_admitted: None,
            },
            reorder_capacity: 4,
        },
        config(2, 8, 4, Some(99)),
    ] {
        assert!(
            go(settings, records.clone(), Arrangement::StagedPipeline).is_err(),
            "invalid configuration must be refused"
        );
    }
    let mut oversized = records.clone();
    oversized[0].publish_work = crate::MAX_WORK + 1;
    assert!(go(config(2, 8, 4, None), oversized, Arrangement::SerialOwner).is_err());
    let mut duplicated = records.clone();
    duplicated[1].id = duplicated[0].id;
    assert!(go(config(2, 8, 4, None), duplicated, Arrangement::SerialOwner).is_err());
    let mut reordered = records;
    reordered[0].at_ns = 5_000;
    reordered[1].at_ns = 1_000;
    assert!(go(config(2, 8, 4, None), reordered, Arrangement::SerialOwner).is_err());
}

#[test]
fn pressure_keeps_admission_inside_the_credit_ceiling() {
    let records = trace(96, 96, 0, 2000, 500, false, 0).unwrap();
    for arrangement in [Arrangement::SerialOwner, Arrangement::StagedPipeline] {
        let report = go(config(2, 2, 1, None), records.clone(), arrangement).unwrap();
        assert_eq!(report.log, expected_log(&records));
        assert!(report.peak_outstanding <= 2);
        assert_eq!(report.request_slots, 2);
    }
}
