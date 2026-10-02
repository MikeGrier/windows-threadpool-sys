// Copyright (c) 2026 Mike Grier
//! Why `prewarm_default_pool` does not check the worker count before submitting.
//!
//! The obvious optimisation is to query the pool first and skip the work item
//! when a worker already exists. **Measured, that is a pessimisation**: the
//! query has to scan the handle space asking each candidate whether it is a
//! worker factory, which costs several times more than the submit it would
//! avoid.
//!
//! This test pins the comparison so that the decision is reviewable rather than
//! remembered. If the relationship ever inverts -- a cheaper way to read the
//! count, or a more expensive submit -- this fails and the decision should be
//! revisited.
//!
//! An integration test for the same reason as `prewarm.rs`: the first call has
//! to land on a pool that has never made a worker, which needs a process of its
//! own.

#![cfg(windows)]
#![cfg(feature = "trace")]

use std::time::Instant;

use windows_threadpool_sys::pool::prewarm_default_pool;
use windows_threadpool_sys::trace::worker_factory_snapshot;

/// Median of a sample, which is what the assertion uses: a single timing on a
/// loaded machine is noise, and the margin being guarded is large.
fn median(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

#[test]
fn querying_the_worker_count_costs_more_than_the_submit_it_would_skip() {
    // The cold call, reported for context. Not asserted on: it creates a
    // thread, so it is a different quantity from the two being compared.
    let t = Instant::now();
    assert!(
        prewarm_default_pool(),
        "the first call must confirm a worker"
    );
    let cold_us = t.elapsed().as_micros();

    // What a skip would avoid: prewarming a pool that is already warm.
    let warm_us = median(
        (0..10)
            .map(|_| {
                let t = Instant::now();
                assert!(prewarm_default_pool(), "a warm pool must still confirm");
                t.elapsed().as_micros()
            })
            .collect(),
    );

    // What a skip would have to pay to decide.
    let query_us = median(
        (0..10)
            .map(|_| {
                let t = Instant::now();
                let snap = worker_factory_snapshot();
                let us = t.elapsed().as_micros();
                assert!(!snap.is_empty(), "the snapshot must find a factory");
                us
            })
            .collect(),
    );

    eprintln!(
        "prewarm: cold={cold_us}us warm={warm_us}us | query={query_us}us \
         (ratio {:.1}x)",
        query_us as f64 / warm_us.max(1) as f64
    );

    assert!(
        query_us > warm_us,
        "the decision not to check the worker count before submitting rests on the query being \
         the more expensive of the two. It is now cheaper (query={query_us}us, \
         warm prewarm={warm_us}us), so revisit prewarm_default_pool."
    );
}
