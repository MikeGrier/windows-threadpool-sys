// Copyright (c) Mike Grier.
//! Tests for the list of configurations `measure` times.

use super::*;

#[test]
fn the_sequential_order_is_the_order_the_published_figures_were_taken_in() {
    // **Written out rather than derived, because it is a record of a procedure
    // that has already run.** Every figure published before `M4.4` was measured
    // in this order, and `--order sequential` is how they are retaken -- which is
    // true only while `configurations()` lists them in it. Nothing else depends
    // on the order, so nothing else would notice it changing.
    let mut published = vec![
        (Regime::Isolated, shapes::BASELINE_FETCH_ADD),
        (Regime::Isolated, shapes::SLOTWISE_MPSC),
        (Regime::Isolated, shapes::RESERVING_MPSC),
        (Regime::Isolated, shapes::PERMIT_MPSC),
        (Regime::Drained, shapes::SLOTWISE_MPSC),
        (Regime::Drained, shapes::RESERVING_MPSC),
        (Regime::Drained, shapes::PERMIT_MPSC),
        (Regime::Isolated, shapes::CLAIM_NARROW),
        (Regime::Isolated, shapes::CLAIM_DEEP),
        (Regime::Isolated, shapes::CLAIM_PERPETUAL),
    ];
    let wide = cfg!(any(
        all(target_arch = "x86_64", target_feature = "cmpxchg16b"),
        target_arch = "aarch64"
    ));
    if wide {
        published.push((Regime::Isolated, shapes::CLAIM_WIDE));
    }
    published.extend([
        (Regime::Drained, shapes::CLAIM_NARROW),
        (Regime::Drained, shapes::CLAIM_DEEP),
        (Regime::Drained, shapes::CLAIM_PERPETUAL),
    ]);
    if wide {
        published.push((Regime::Drained, shapes::CLAIM_WIDE));
    }

    let listed: Vec<(Regime, &'static str)> = configurations()
        .into_iter()
        .map(|(regime, shape, _)| (regime, shape))
        .collect();
    assert_eq!(listed, published);
}
