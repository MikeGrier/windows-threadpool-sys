// Copyright (c) Mike Grier.

//! Tests for the counters in isolation, with no queue attached.
//!
//! Their behaviour *through* a queue is asserted in each shape's own suite,
//! because each records depth from a different place -- and on `mpsc` records
//! it only when asked. What is tested here is the arithmetic they share.

use super::Metrics;

#[test]
fn refusals_start_at_zero_and_accumulate() {
    let metrics = Metrics::new(false);
    assert_eq!(metrics.refused(), 0);

    for expected in 1..=5 {
        metrics.record_refusal();
        assert_eq!(metrics.refused(), expected);
    }
}

#[test]
fn an_untracked_high_water_reports_none_rather_than_zero() {
    // The distinction the `Option` exists to draw. A caller sizing a queue from
    // `Some(0)` would conclude it never filled; from `None` it learns that
    // nobody was counting, which is a different fact and demands a different
    // response.
    let metrics = Metrics::new(false);
    assert!(!metrics.tracks_high_water());
    assert_eq!(metrics.high_water(), None);

    // And recording into it is a no-op rather than an error, so the shapes can
    // call it unconditionally where the depth is free.
    metrics.record_depth(9);
    assert_eq!(metrics.high_water(), None);
}

#[test]
fn a_tracked_high_water_starts_at_some_zero() {
    // Distinct from `None`: this queue *is* counting, and has seen nothing.
    let metrics = Metrics::new(true);
    assert!(metrics.tracks_high_water());
    assert_eq!(metrics.high_water(), Some(0));
}

#[test]
fn high_water_keeps_the_peak_rather_than_the_latest() {
    let metrics = Metrics::new(true);

    metrics.record_depth(3);
    assert_eq!(metrics.high_water(), Some(3));

    metrics.record_depth(7);
    assert_eq!(metrics.high_water(), Some(7));

    // The point of a high-water mark: it does not fall when the queue drains.
    metrics.record_depth(1);
    assert_eq!(
        metrics.high_water(),
        Some(7),
        "a peak that receded is still a peak that happened"
    );

    metrics.record_depth(7);
    assert_eq!(metrics.high_water(), Some(7), "and equal is not greater");
}

#[test]
fn concurrent_recorders_do_not_lose_the_peak() {
    // `record_depth` loads before it modifies, so two threads can both observe
    // a stale maximum. The `fetch_max` that follows is what makes the result
    // correct anyway, and this is the test that says so: without it, the
    // load-then-modify shortcut would be a lost update rather than an
    // optimisation.
    use std::sync::Arc;
    use std::thread;

    const THREADS: usize = 4;
    const PER_THREAD: usize = 500;

    let metrics = Arc::new(Metrics::new(true));
    let threads: Vec<_> = (0..THREADS)
        .map(|offset| {
            let metrics = Arc::clone(&metrics);
            thread::spawn(move || {
                for depth in 0..PER_THREAD {
                    metrics.record_depth(depth + offset * PER_THREAD);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("no recorder may panic");
    }

    assert_eq!(
        metrics.high_water(),
        Some(THREADS * PER_THREAD - 1),
        "the largest value any thread recorded must survive every race"
    );
}

#[test]
fn the_debug_form_reports_both_counters() {
    let metrics = Metrics::new(true);
    metrics.record_refusal();
    metrics.record_depth(4);

    let shown = format!("{metrics:?}");
    assert!(shown.contains("refused: 1"), "{shown}");
    assert!(shown.contains("Some(4)"), "{shown}");
}
