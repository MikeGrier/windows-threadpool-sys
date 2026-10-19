// Copyright (c) Mike Grier.
//! Tests for the queue probe's settings (`M4.2`): what a command line asks for, what is refused,
//! and that the median and the per-operation figures follow the sampling a run was given.

use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

/// A timer that hands back `values` in order, the first to the untimed warmup pass.
fn scripted(values: Vec<Repetition>) -> impl FnMut(usize, usize) -> Repetition {
    let mut values = values.into_iter();
    move |_producers, _pushes| values.next().expect("a scripted pass for every call")
}

#[test]
fn no_settings_sample_exactly_as_the_published_figures_did() {
    let sampling = Sampling::from_args(args(&[])).expect("nothing to refuse");
    assert_eq!(sampling, Sampling::default());
    assert!(sampling.is_default());
    assert_eq!(sampling.pushes_per_producer, PUSHES_PER_PRODUCER);
    assert_eq!(sampling.repetitions, REPETITIONS);
    assert_eq!(sampling.producer_counts, PRODUCER_COUNTS);
}

#[test]
fn each_setting_replaces_its_default_and_leaves_the_others() {
    let sampling = Sampling::from_args(args(&["--repetitions", "9"])).expect("valid");
    assert_eq!(sampling.repetitions, 9);
    assert_eq!(sampling.pushes_per_producer, PUSHES_PER_PRODUCER);
    assert_eq!(sampling.producer_counts, PRODUCER_COUNTS);
    assert!(!sampling.is_default());

    let sampling = Sampling::from_args(args(&[
        "--pushes-per-producer=200000",
        "--producers",
        "4,1",
    ]))
    .expect("valid");
    assert_eq!(sampling.pushes_per_producer, 200_000);
    assert_eq!(sampling.repetitions, REPETITIONS);
    assert_eq!(
        sampling.producer_counts,
        vec![4, 1],
        "the order given is kept"
    );
}

#[test]
fn a_setting_given_its_default_value_is_still_the_default_run() {
    let sampling = Sampling::from_args(args(&["--repetitions", "5"])).expect("valid");
    assert!(sampling.is_default());
}

#[test]
fn a_setting_the_probe_cannot_use_is_refused_with_its_flag() {
    for (arguments, flag) in [
        (&["--repetitions", "0"][..], "repetitions"),
        (&["--pushes-per-producer", "x"][..], "pushes-per-producer"),
        (&["--producers", "1,1"][..], "producers"),
        (&["--producers", ""][..], "producers"),
    ] {
        let error = Sampling::from_args(args(arguments)).expect_err("refused");
        assert!(
            matches!(&error, ArgError::Invalid { flag: refused, .. } if *refused == flag),
            "{arguments:?}: {error:?}"
        );
    }
    assert_eq!(
        Sampling::from_args(args(&["--iteration-multiplier", "2"])),
        Err(ArgError::Unknown("--iteration-multiplier".to_owned())),
        "the cost probes' setting is not this probe's"
    );
}

#[test]
fn a_run_too_large_to_size_a_queue_for_is_refused_before_it_starts() {
    let pushes = (usize::MAX / 2).to_string();
    let error = Sampling::from_args(args(&[
        "--pushes-per-producer",
        &pushes,
        "--producers",
        "1,4",
    ]))
    .expect_err("too large");
    assert!(
        error
            .to_string()
            .contains("4 producers times this many pushes"),
        "{error}"
    );

    // The largest producer count decides, not the first: the same order of pushes
    // with one producer sizes a queue.
    let fits = (usize::MAX / 4).to_string();
    assert!(
        Sampling::from_args(args(&["--pushes-per-producer", &fits, "--producers", "1"])).is_ok()
    );
}

#[test]
fn the_median_follows_the_repetition_count_it_was_given() {
    // A warmup pass and then three timed passes of 3, 1 and 2 ms: the median is the
    // middle one and the extremes are the ends -- of three, not the five the
    // default takes.
    let three = Sampling {
        repetitions: 3,
        ..Sampling::default()
    };
    let measured = median_run(
        shapes::RESERVING_MPSC,
        1,
        &three,
        scripted(vec![(0.0, 0), (3e6, 0), (1e6, 0), (2e6, 0)]),
    );
    let pushes = PUSHES_PER_PRODUCER as f64;
    assert_eq!(measured.nanos_per_op, 2e6 / pushes);
    assert_eq!(measured.fastest_nanos_per_op, 1e6 / pushes);
    assert_eq!(measured.slowest_nanos_per_op, 3e6 / pushes);
}

#[test]
fn with_an_even_count_the_median_is_the_upper_of_the_two_middle_passes() {
    let four = Sampling {
        repetitions: 4,
        ..Sampling::default()
    };
    let measured = median_run(
        shapes::RESERVING_MPSC,
        1,
        &four,
        scripted(vec![(0.0, 0), (4e6, 0), (1e6, 0), (3e6, 0), (2e6, 0)]),
    );
    assert_eq!(measured.nanos_per_op, 3e6 / PUSHES_PER_PRODUCER as f64);
}

#[test]
fn every_pass_is_asked_for_the_producers_and_the_pushes_the_run_was_given() {
    let sampling = Sampling {
        pushes_per_producer: 777,
        repetitions: 3,
        ..Sampling::default()
    };
    let mut asked = Vec::new();
    let _ = median_run(shapes::SLOTWISE_MPSC, 4, &sampling, |producers, pushes| {
        asked.push((producers, pushes));
        (1e6, 0)
    });
    assert_eq!(
        asked,
        vec![(4, 777); 4],
        "a warmup pass and three timed ones"
    );
}

#[test]
fn the_per_operation_figures_divide_by_the_pushes_the_run_used() {
    let short = Sampling {
        pushes_per_producer: 1_000,
        repetitions: 1,
        ..Sampling::default()
    };
    let measured = median_run(
        shapes::RESERVING_MPSC,
        2,
        &short,
        scripted(vec![(0.0, 0), (4e6, 0)]),
    );
    assert_eq!(measured.nanos_per_op, 4e6 / 2_000.0);
    assert_eq!(measured.ops_per_second, 2_000.0 / (4e6 / 1e9));
}

#[test]
fn a_short_run_measures_every_shape_at_every_count_it_was_given() {
    // A real measurement, kept tiny: one repetition of a few pushes at two
    // producer counts. It shows `measure` walks the counts it was given, in that
    // order, and records the sampling it used -- not that any figure means
    // anything.
    let sampling = Sampling {
        pushes_per_producer: 64,
        repetitions: 1,
        producer_counts: vec![2, 1],
    };
    let observation = measure(&sampling);
    assert_eq!(observation.sampling, sampling);
    let counts: Vec<usize> = observation
        .isolated
        .iter()
        .filter(|run| run.shape == shapes::RESERVING_MPSC)
        .map(|run| run.producers)
        .collect();
    assert_eq!(counts, vec![2, 1]);
    assert!(
        observation
            .isolated
            .iter()
            .chain(&observation.drained)
            .all(|run| run.producers == 1 || run.producers == 2)
    );
}
