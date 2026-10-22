// Copyright (c) Mike Grier.
//! Tests for the queue probe's settings (`M4.2`, and `M4.4`'s order): what a command line asks
//! for, what is refused, and that the median, the per-operation figures and the order of the
//! passes follow the sampling a run was given.

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
fn no_settings_take_the_published_counts_interleaved() {
    let sampling = Sampling::from_args(args(&[])).expect("nothing to refuse");
    assert_eq!(sampling, Sampling::default());
    assert!(sampling.is_default());
    assert_eq!(sampling.pushes_per_producer, PUSHES_PER_PRODUCER);
    assert_eq!(sampling.repetitions, REPETITIONS);
    assert_eq!(sampling.producer_counts, PRODUCER_COUNTS);
    assert_eq!(sampling.order, Order::Interleaved);
}

#[test]
fn the_order_is_set_by_name_and_anything_else_is_refused() {
    for order in [Order::Interleaved, Order::Sequential] {
        let sampling = Sampling::from_args(args(&["--order", order.name()])).expect("valid");
        assert_eq!(sampling.order, order);
        assert_eq!(sampling.is_default(), order == Order::Interleaved);
        assert_eq!(Order::from_name(order.name()), Some(order));
    }
    for refused in ["random", "Interleaved", ""] {
        let error = Sampling::from_args(args(&["--order", refused])).expect_err("refused");
        assert!(
            matches!(&error, ArgError::Invalid { flag: "order", .. }),
            "{refused:?}: {error:?}"
        );
    }
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

/// Timers that log which of them was called, in call order, and hand back what
/// `elapsed` says for that call.
fn logging_timers<'a>(
    count: usize,
    log: &'a std::cell::RefCell<Vec<usize>>,
    elapsed: impl Fn(usize) -> f64 + Copy + 'a,
) -> Vec<(&'static str, Timer<'a>)> {
    (0..count)
        .map(|index| {
            let timer: Timer<'a> = Box::new(move |_producers, _pushes| {
                let call = log.borrow().len();
                log.borrow_mut().push(index);
                (elapsed(call), 0)
            });
            (shapes::RESERVING_MPSC, timer)
        })
        .collect()
}

#[test]
fn interleaved_takes_every_warmup_then_rotates_the_first_position_each_round() {
    let sampling = Sampling {
        repetitions: 3,
        ..Sampling::default()
    };
    let log = std::cell::RefCell::new(Vec::new());
    let mut timers = logging_timers(3, &log, |_| 1e6);
    let measured = runs(1, &sampling, &mut timers);

    assert_eq!(
        *log.borrow(),
        vec![0, 1, 2, 0, 1, 2, 1, 2, 0, 2, 0, 1],
        "three warmups in list order, then rounds starting at 0, 1 and 2"
    );
    assert_eq!(measured.len(), 3, "one run per configuration");
}

#[test]
fn sequential_takes_each_configurations_passes_back_to_back() {
    let sampling = Sampling {
        repetitions: 2,
        order: Order::Sequential,
        ..Sampling::default()
    };
    let log = std::cell::RefCell::new(Vec::new());
    let mut timers = logging_timers(3, &log, |_| 1e6);
    let _ = runs(1, &sampling, &mut timers);

    assert_eq!(
        *log.borrow(),
        vec![0, 0, 0, 1, 1, 1, 2, 2, 2],
        "a warmup and two passes each, one configuration after another"
    );
}

#[test]
fn each_run_is_the_median_of_its_own_passes_whatever_the_order() {
    // Configuration `k`'s passes take `k` milliseconds plus the pass number in
    // nanoseconds, so a run that took another configuration's passes would show it.
    for order in [Order::Interleaved, Order::Sequential] {
        let sampling = Sampling {
            repetitions: 3,
            pushes_per_producer: 1,
            order,
            ..Sampling::default()
        };
        let calls = std::cell::RefCell::new(vec![0usize; 4]);
        let mut timers: Vec<(&'static str, Timer<'_>)> = (0..4)
            .map(|k| {
                let calls = &calls;
                let timer: Timer<'_> = Box::new(move |_producers, _pushes| {
                    let pass = calls.borrow()[k];
                    calls.borrow_mut()[k] += 1;
                    ((k as f64) * 1e6 + pass as f64, 0)
                });
                (shapes::RESERVING_MPSC, timer)
            })
            .collect();
        let measured = runs(1, &sampling, &mut timers);
        let medians: Vec<f64> = measured.iter().map(|run| run.nanos_per_op).collect();
        // Pass 0 is the warmup, so the timed passes are 1, 2 and 3 and the median 2.
        assert_eq!(
            medians,
            vec![2.0, 1e6 + 2.0, 2e6 + 2.0, 3e6 + 2.0],
            "{order:?}"
        );
    }
}

#[test]
fn interleaving_puts_two_halves_of_a_control_inside_one_interval_under_drift() {
    // **The property `M4.4` exists for, under a drift model rather than on a
    // machine.** Every pass takes as long as the number of passes before it, so
    // the run slows steadily. Two configurations measuring the same code then
    // differ only by when they were measured -- which is the control's job to
    // show as nothing at all.
    //
    // Fifteen configurations with the halves at positions 2 and 7, as in the
    // isolated regime. Back to back, the halves' medians sit five configurations
    // of (warmup + passes) apart; interleaved, they sit less than one round apart.
    let count = 15;
    let (first, second) = (2, 7);
    let median_gap = |order: Order| {
        let sampling = Sampling {
            repetitions: 5,
            pushes_per_producer: 1,
            order,
            ..Sampling::default()
        };
        let log = std::cell::RefCell::new(Vec::new());
        let mut timers = logging_timers(count, &log, |call| call as f64);
        let measured = runs(1, &sampling, &mut timers);
        measured[second].nanos_per_op - measured[first].nanos_per_op
    };

    let sequential = median_gap(Order::Sequential);
    let interleaved = median_gap(Order::Interleaved);
    assert_eq!(
        sequential,
        ((second - first) * (1 + 5)) as f64,
        "back to back, the gap is every pass taken between the two"
    );
    assert!(
        interleaved.abs() < count as f64,
        "interleaved, the gap is less than one round: {interleaved}"
    );
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
        order: Order::Interleaved,
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

    // The order changes when passes are taken, never which rows a run reports or
    // the table each lands in.
    let sequential = measure(&Sampling {
        order: Order::Sequential,
        ..sampling
    });
    let rows = |runs: &[Run]| -> Vec<(&'static str, usize)> {
        runs.iter().map(|run| (run.shape, run.producers)).collect()
    };
    assert_eq!(rows(&sequential.isolated), rows(&observation.isolated));
    assert_eq!(rows(&sequential.drained), rows(&observation.drained));
}
