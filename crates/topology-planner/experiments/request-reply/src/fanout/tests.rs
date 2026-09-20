// Copyright (c) Mike Grier.
use super::*;
use std::sync::atomic::AtomicBool;

fn settings() -> Config {
    Config {
        workers: 3,
        capacity: 6,
        timeout_ms: 1000,
        cancel_after_admitted: None,
    }
}

fn sample(arrangement: Arrangement, fail_every: usize) -> Report {
    let records = trace(TraceSpec {
        count: 12,
        burst: 4,
        work: 200,
        units: 5,
        broadcast_every: 3,
        skew_every: 2,
        fail_every,
        ..Default::default()
    })
    .unwrap();
    run(settings(), records, arrangement, &AtomicBool::new(false)).unwrap()
}

fn at(report: &Report, position: usize) -> usize {
    report
        .completions
        .iter()
        .position(|completion| completion.position == position)
        .unwrap()
}

#[test]
fn a_real_report_from_every_arrangement_verifies() {
    for arrangement in [
        Arrangement::SerialWhole,
        Arrangement::OwnedJoin,
        Arrangement::ScatteredJoin,
    ] {
        for fail_every in [0, 4] {
            verify(&sample(arrangement, fail_every)).unwrap();
        }
    }
}

#[test]
fn membership_must_be_exact_in_both_directions() {
    let good = sample(Arrangement::ScatteredJoin, 0);
    let index = at(&good, 3);

    let mut missing = good.clone();
    missing.completions[index].units.pop();
    assert!(verify(&missing).is_err(), "a missing unit must be rejected");

    let mut duplicated = good.clone();
    let repeat = duplicated.completions[index].units[0];
    duplicated.completions[index].units.push(repeat);
    assert!(
        verify(&duplicated).is_err(),
        "a unit counted twice must be rejected"
    );

    let mut foreign = good.clone();
    foreign.completions[index].units[0].index = 99;
    assert!(
        verify(&foreign).is_err(),
        "a unit outside the record's fan-out must be rejected"
    );
}

#[test]
fn the_joined_value_and_log_must_match_the_serial_replay() {
    let good = sample(Arrangement::OwnedJoin, 0);

    let mut drifted = good.clone();
    let index = at(&drifted, 2);
    drifted.completions[index].outcome = Outcome::Joined { value: 1 };
    assert!(verify(&drifted).is_err(), "a drifted join value");

    let mut swapped = good.clone();
    swapped.log.swap(0, 1);
    assert!(verify(&swapped).is_err(), "a reordered join log");

    let mut altered = good.clone();
    altered.log[2].value = altered.log[2].value.wrapping_add(1);
    assert!(verify(&altered).is_err(), "a wrong log value");

    let mut extra = good.clone();
    extra.log.push(JoinedRecord { id: 999, value: 0 });
    assert!(verify(&extra).is_err(), "an extra log entry");
}

#[test]
fn the_declared_shape_is_bound_into_the_oracle() {
    let mut flipped = sample(Arrangement::SerialWhole, 0);
    let record = flipped
        .trace
        .iter_mut()
        .find(|record| matches!(record.shape, Shape::Scatter { .. }))
        .unwrap();
    let units = record.units();
    record.shape = Shape::Broadcast { branches: units };
    assert!(
        verify(&flipped).is_err(),
        "reading a scatter record as a broadcast must not still verify"
    );
}

#[test]
fn owner_routing_is_checked_only_where_the_arrangement_declares_it() {
    let mut owned = sample(Arrangement::OwnedJoin, 0);
    let index = at(&owned, 1);
    let wrong = (owned.completions[index].joiner + 1) % owned.config.workers;
    owned.completions[index].joiner = wrong;
    assert!(
        verify(&owned).is_err(),
        "an owned join assembled elsewhere must be rejected"
    );

    let mut units = sample(Arrangement::OwnedJoin, 0);
    let index = at(&units, 1);
    units.completions[index].units[0].worker =
        (units.completions[index].units[0].worker + 1) % units.config.workers;
    assert!(
        verify(&units).is_err(),
        "a unit computed off its owner must be rejected"
    );

    let mut serial = sample(Arrangement::SerialWhole, 0);
    let index = at(&serial, 1);
    serial.completions[index].units[0].worker = 0;
    verify(&serial).unwrap();
}

#[test]
fn unit_timestamps_must_lie_inside_their_record_span() {
    let good = sample(Arrangement::ScatteredJoin, 0);
    let index = at(&good, 4);
    for mutate in [
        (|completion: &mut Completion| completion.units[0].started_ns -= 1) as fn(&mut Completion),
        |completion| completion.units[0].finished_ns += 1,
        |completion| completion.started_ns += 1,
        |completion| completion.gathered_ns -= 1,
        |completion| completion.joined_ns = completion.gathered_ns - 1,
        |completion| completion.collected_ns = completion.joined_ns - 1,
    ] {
        let mut broken = good.clone();
        mutate(&mut broken.completions[index]);
        assert!(
            verify(&broken).is_err(),
            "a unit or record timestamp outside its span must be rejected"
        );
    }
}

#[test]
fn abort_cancellation_and_reclamation_are_checked() {
    let good = sample(Arrangement::ScatteredJoin, 4);
    let aborted = good
        .completions
        .iter()
        .position(|completion| matches!(completion.outcome, Outcome::Aborted { .. }))
        .unwrap();

    let mut joined = good.clone();
    joined.completions[aborted].outcome = Outcome::Joined { value: 0 };
    assert!(
        verify(&joined).is_err(),
        "a record with an injected failure must not join"
    );

    let mut elsewhere = good.clone();
    let unit = match good.completions[aborted].outcome {
        Outcome::Aborted { unit } => unit,
        _ => unreachable!(),
    };
    elsewhere.completions[aborted].outcome = Outcome::Aborted {
        unit: (unit + 1) % good.completions[aborted].units.len() as u32,
    };
    assert!(
        verify(&elsewhere).is_err(),
        "an abort must name the unit the trace injected"
    );

    let mut counted = good.clone();
    counted.discarded_partial_units += 1;
    assert!(verify(&counted).is_err(), "discard census mismatch");

    let mut residual = good.clone();
    residual.residual_partial_units = 1;
    assert!(
        verify(&residual).is_err(),
        "partial state left behind must be rejected"
    );

    let mut drained = good;
    let last = at(&drained, drained.trace.len() - 1);
    drained.completions[last].outcome = Outcome::Cancelled;
    assert!(
        verify(&drained).is_err(),
        "a drained run cannot report a cancelled record"
    );
}

#[test]
fn census_credit_and_worker_mismatches_are_rejected() {
    let good = sample(Arrangement::ScatteredJoin, 0);

    let mut slots = good.clone();
    slots.unit_slots += 1;
    assert!(verify(&slots).is_err(), "unit slot census");

    let mut partial = good.clone();
    partial.peak_partial_records = partial.config.capacity + 1;
    assert!(verify(&partial).is_err(), "partial records beyond credits");

    let mut credits = good.clone();
    credits.credit_events.pop();
    assert!(verify(&credits).is_err(), "unbalanced credits");

    let mut peak = good.clone();
    peak.peak_outstanding += 1;
    assert!(verify(&peak).is_err(), "peak outstanding mismatch");

    let mut counted = good.clone();
    counted.workers[0].units += 1;
    assert!(verify(&counted).is_err(), "unit census mismatch");

    let mut roles = good.clone();
    roles.workers[0].role = Role::Joiner;
    assert!(verify(&roles).is_err(), "role census mismatch");

    let mut held = good.clone();
    held.workers[0].peak_units_held = 1;
    assert!(
        verify(&held).is_err(),
        "a unit worker holds no partial state"
    );

    let mut trimmed = good;
    trimmed.workers.pop();
    assert!(verify(&trimmed).is_err(), "thread census mismatch");
}

#[test]
fn traces_and_summaries_respect_the_declared_contract() {
    assert!(
        trace(TraceSpec {
            count: 4,
            work: 100,
            units: 0,
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        trace(TraceSpec {
            count: 4,
            work: 100,
            units: MAX_UNITS + 1,
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        trace(TraceSpec {
            count: 4,
            work: crate::MAX_WORK + 1,
            units: 2,
            ..Default::default()
        })
        .is_err()
    );

    let records = trace(TraceSpec {
        count: 16,
        burst: 4,
        work: 100,
        units: 4,
        broadcast_every: 2,
        ..Default::default()
    })
    .unwrap();
    assert!(
        records
            .iter()
            .any(|record| matches!(record.shape, Shape::Broadcast { .. }))
    );
    assert!(
        records
            .iter()
            .any(|record| matches!(record.shape, Shape::Scatter { .. }))
    );

    let report = sample(Arrangement::ScatteredJoin, 0);
    let all = summary(&report, None).unwrap();
    let scattered = summary(&report, Some(false)).unwrap();
    let broadcast = summary(&report, Some(true)).unwrap();
    assert_eq!(all.joined.count, report.log.len());
    assert_eq!(
        scattered.units_per_record.count + broadcast.units_per_record.count,
        all.units_per_record.count
    );
}
