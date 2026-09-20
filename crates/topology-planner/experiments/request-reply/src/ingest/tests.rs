// Copyright (c) Mike Grier.
use super::*;
use std::sync::atomic::AtomicBool;

fn settings(reorder: usize) -> Config {
    Config {
        limits: Limits {
            workers: 2,
            capacity: 8,
            timeout_ms: 1000,
            cancel_after_admitted: None,
        },
        reorder_capacity: reorder,
    }
}

fn sample(arrangement: Arrangement, fail_every: usize) -> Report {
    let records = trace(12, 4, 0, 400, 200, false, fail_every).unwrap();
    run(settings(4), records, arrangement, &AtomicBool::new(false)).unwrap()
}

fn position_of(report: &Report, position: usize) -> usize {
    report
        .completions
        .iter()
        .position(|completion| completion.position == position)
        .unwrap()
}

#[test]
fn a_real_report_from_either_arrangement_verifies() {
    for arrangement in [Arrangement::SerialOwner, Arrangement::StagedPipeline] {
        for fail_every in [0, 3] {
            verify(&sample(arrangement, fail_every)).unwrap();
        }
    }
}

#[test]
fn the_publication_log_must_match_the_serial_replay_in_order_and_value() {
    let good = sample(Arrangement::StagedPipeline, 0);

    let mut swapped = good.clone();
    swapped.log.swap(0, 1);
    assert!(verify(&swapped).is_err(), "reordered log must be rejected");

    let mut altered = good.clone();
    altered.log[2].value = altered.log[2].value.wrapping_add(1);
    assert!(verify(&altered).is_err(), "wrong value must be rejected");

    let mut dropped = good.clone();
    dropped.log.pop();
    assert!(verify(&dropped).is_err(), "missing entry must be rejected");

    let mut extra = good.clone();
    extra.log.push(Published { id: 999, value: 0 });
    assert!(verify(&extra).is_err(), "extra entry must be rejected");
}

#[test]
fn an_aborted_record_must_publish_nothing_and_name_its_injected_stage() {
    let good = sample(Arrangement::SerialOwner, 3);
    let aborted = good
        .completions
        .iter()
        .position(|completion| matches!(completion.outcome, Outcome::Aborted { .. }))
        .unwrap();

    let mut published = good.clone();
    published.completions[aborted].outcome = Outcome::Published { value: 0 };
    assert!(
        verify(&published).is_err(),
        "a record with an injected failure must not report publication"
    );

    let mut wrong_stage = good.clone();
    let stage = match good.completions[aborted].outcome {
        Outcome::Aborted { stage } => stage,
        _ => unreachable!(),
    };
    wrong_stage.completions[aborted].outcome = Outcome::Aborted {
        stage: match stage {
            Stage::Transform => Stage::Publish,
            Stage::Publish => Stage::Transform,
        },
    };
    assert!(
        verify(&wrong_stage).is_err(),
        "an abort must name the stage the trace injected"
    );
}

#[test]
fn a_cancelled_outcome_requires_a_stop_and_ends_publication() {
    let good = sample(Arrangement::StagedPipeline, 0);

    let mut drained = good.clone();
    let last = position_of(&drained, drained.trace.len() - 1);
    drained.completions[last].outcome = Outcome::Cancelled;
    assert!(
        verify(&drained).is_err(),
        "a drained run cannot report a cancelled record"
    );

    let mut resumed = good.clone();
    resumed.stop = StopReason::Cancelled;
    let middle = position_of(&resumed, 4);
    resumed.completions[middle].outcome = Outcome::Cancelled;
    assert!(
        verify(&resumed).is_err(),
        "publication must not resume after a cancelled record"
    );

    let mut aborting = sample(Arrangement::SerialOwner, 3);
    aborting.stop = StopReason::Cancelled;
    let earlier = position_of(&aborting, 0);
    aborting.completions[earlier].outcome = Outcome::Cancelled;
    assert!(
        verify(&aborting).is_err(),
        "an abort must not follow a cancelled record either"
    );
}

#[test]
fn stage_timestamps_must_respect_each_record_s_own_dependency() {
    // Spaced arrivals, so scheduled_ns is nonzero and an offered-time inversion is expressible.
    let records = trace(12, 1, 1000, 400, 200, false, 0).unwrap();
    let good = run(
        settings(4),
        records,
        Arrangement::StagedPipeline,
        &AtomicBool::new(false),
    )
    .unwrap();
    let index = position_of(&good, 3);
    assert!(good.completions[index].scheduled_ns > 0);
    for mutate in [
        (|completion: &mut Completion| completion.started_ns = completion.admitted_ns - 1)
            as fn(&mut Completion),
        |completion| completion.ready_ns = completion.started_ns - 1,
        |completion| completion.published_ns = completion.ready_ns - 1,
        |completion| completion.finished_ns = completion.published_ns - 1,
        |completion| completion.collected_ns = completion.finished_ns - 1,
        |completion| completion.offered_ns = completion.scheduled_ns - 1,
    ] {
        let mut broken = good.clone();
        mutate(&mut broken.completions[index]);
        assert!(
            verify(&broken).is_err(),
            "an inverted stage timestamp must be rejected"
        );
    }
}

#[test]
fn identity_position_and_census_mismatches_are_rejected() {
    let good = sample(Arrangement::StagedPipeline, 0);

    let mut shifted = good.clone();
    shifted.completions[0].position += 1;
    assert!(verify(&shifted).is_err(), "declared position must hold");

    let mut duplicated = good.clone();
    duplicated.completions[1].id = duplicated.completions[0].id;
    assert!(verify(&duplicated).is_err(), "duplicate identity");

    let mut stranger = good.clone();
    stranger.completions[0].worker = 99;
    assert!(verify(&stranger).is_err(), "worker outside the census");

    let mut slots = good.clone();
    slots.request_slots += 1;
    assert!(verify(&slots).is_err(), "request slot census");

    let mut window = good.clone();
    window.peak_reorder_occupancy = window.reorder_slots + 1;
    assert!(verify(&window).is_err(), "occupancy outside the window");

    let mut serial = sample(Arrangement::SerialOwner, 0);
    serial.reorder_full_observations = 1;
    assert!(
        verify(&serial).is_err(),
        "a serial owner has no reorder buffer to block on"
    );
}

#[test]
fn credit_conservation_and_worker_counts_are_checked() {
    let good = sample(Arrangement::StagedPipeline, 0);

    let mut missing = good.clone();
    missing.credit_events.pop();
    assert!(verify(&missing).is_err(), "unbalanced credits");

    let mut peak = good.clone();
    peak.peak_outstanding += 1;
    assert!(verify(&peak).is_err(), "peak outstanding mismatch");

    let mut counted = good.clone();
    counted.workers[0].transformed += 1;
    assert!(verify(&counted).is_err(), "transform census mismatch");

    let mut roles = good.clone();
    roles.workers[0].role = Role::Publisher;
    assert!(verify(&roles).is_err(), "role census mismatch");

    let mut active = good.clone();
    active.workers[0].active_ns += 1;
    assert!(verify(&active).is_err(), "active time census mismatch");

    let mut trimmed = good;
    trimmed.workers.pop();
    assert!(verify(&trimmed).is_err(), "thread census mismatch");
}

#[test]
fn trace_and_summary_reject_arguments_outside_the_contract() {
    assert!(trace(4, 1, 0, crate::MAX_WORK + 1, 0, false, 0).is_err());
    assert!(trace(4, 1, 0, 0, crate::MAX_WORK + 1, false, 0).is_err());
    assert!(trace(4, 0, 0, 100, 100, false, 0).is_err());

    let report = sample(Arrangement::StagedPipeline, 0);
    assert!(summary(&report, Some(2)).is_err());
    let aggregate = summary(&report, None).unwrap();
    assert_eq!(aggregate.published.count, report.log.len());
    for worker in 0..transformers(&report.config, report.arrangement) {
        summary(&report, Some(worker)).unwrap();
    }

    let serial = sample(Arrangement::SerialOwner, 0);
    assert!(summary(&serial, Some(1)).is_err());
    summary(&serial, Some(0)).unwrap();
}

#[test]
fn injected_failures_alternate_between_the_two_stage_boundaries() {
    let records = trace(12, 1, 0, 100, 100, false, 1).unwrap();
    assert_eq!(records[0].fail_at, Some(Stage::Transform));
    assert_eq!(records[1].fail_at, Some(Stage::Publish));
    assert!(records.iter().all(|record| record.fail_at.is_some()));
    let none = trace(12, 1, 0, 100, 100, false, 0).unwrap();
    assert!(none.iter().all(|record| record.fail_at.is_none()));
}
