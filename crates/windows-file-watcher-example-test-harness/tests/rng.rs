// Copyright (c) 2026 Mike Grier
//! Direct coverage of `Rng`'s public sampling methods (PR #42 review): no
//! existing test called them directly, leaving boundary contracts --
//! including the full-width `range` branch that guards against overflow --
//! unverified.

#![cfg(windows)]

use windows_file_watcher_example_test_harness::Rng;

#[test]
fn the_same_seed_always_yields_the_same_sequence() {
    let mut a = Rng::new(42);
    let mut b = Rng::new(42);
    for _ in 0..100 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
}

#[test]
fn different_seeds_diverge() {
    let mut a = Rng::new(1);
    let mut b = Rng::new(2);
    assert_ne!(a.next_u64(), b.next_u64());
}

#[test]
fn below_a_singleton_bound_always_returns_zero() {
    let mut rng = Rng::new(7);
    for _ in 0..20 {
        assert_eq!(rng.below(1), 0);
    }
}

#[test]
#[should_panic(expected = "Rng::below(0)")]
fn below_zero_panics() {
    Rng::new(0).below(0);
}

#[test]
fn below_stays_in_bounds_across_many_draws() {
    let mut rng = Rng::new(123);
    for _ in 0..10_000 {
        let n = rng.below(37);
        assert!(n < 37);
    }
}

#[test]
fn range_a_singleton_bound_always_returns_that_value() {
    let mut rng = Rng::new(9);
    for _ in 0..20 {
        assert_eq!(rng.range(5, 5), 5);
    }
}

#[test]
#[should_panic(expected = "Rng::range with low > high")]
fn range_with_low_greater_than_high_panics() {
    Rng::new(0).range(5, 4);
}

#[test]
fn range_stays_within_bounds_across_many_draws() {
    let mut rng = Rng::new(55);
    for _ in 0..10_000 {
        let n = rng.range(3, 9);
        assert!((3..=9).contains(&n));
    }
}

#[test]
fn range_full_width_never_overflows_and_covers_the_type() {
    // The one branch this generator's own review called out: low == 0,
    // high == u64::MAX makes `high - low + 1` overflow to 0, which range()
    // must detect and treat as "every value is in range" rather than divide
    // by a wrapped-to-zero span. A uniform full-width draw sets its top bit
    // roughly half the time either way, which a handful of draws reliably
    // exercises -- unlike waiting to land within some narrow band near either
    // literal end of u64, which a uniform 64-bit draw would need
    // astronomically many samples to hit.
    let mut rng = Rng::new(999);
    let mut saw_high_bit_set = false;
    let mut saw_high_bit_clear = false;
    for _ in 0..1_000 {
        let n = rng.range(0, u64::MAX);
        if n & (1 << 63) != 0 {
            saw_high_bit_set = true;
        } else {
            saw_high_bit_clear = true;
        }
    }
    assert!(
        saw_high_bit_set && saw_high_bit_clear,
        "1,000 full-width draws should cover both halves of u64"
    );
}

#[test]
fn chance_zero_percent_is_always_false() {
    let mut rng = Rng::new(3);
    for _ in 0..1_000 {
        assert!(!rng.chance(0));
    }
}

#[test]
fn chance_one_hundred_percent_is_always_true() {
    let mut rng = Rng::new(4);
    for _ in 0..1_000 {
        assert!(rng.chance(100));
    }
}

#[test]
fn chance_above_one_hundred_percent_clamps_to_always_true() {
    let mut rng = Rng::new(5);
    for _ in 0..1_000 {
        assert!(rng.chance(255));
    }
}

#[test]
fn chance_fifty_percent_lands_roughly_in_the_middle() {
    let mut rng = Rng::new(6);
    let hits = (0..10_000).filter(|_| rng.chance(50)).count();
    assert!(
        (4_000..6_000).contains(&hits),
        "expected roughly half of 10,000 draws to hit at 50%, got {hits}"
    );
}

#[test]
#[should_panic(expected = "Rng::weighted with zero total weight")]
fn weighted_with_zero_total_weight_panics() {
    Rng::new(0).weighted(&[0, 0, 0]);
}

#[test]
fn weighted_a_single_nonzero_weight_always_wins() {
    let mut rng = Rng::new(11);
    for _ in 0..100 {
        assert_eq!(rng.weighted(&[0, 7, 0]), 1);
    }
}

#[test]
fn weighted_distribution_roughly_matches_the_weights() {
    let mut rng = Rng::new(12);
    let mut counts = [0u32; 3];
    for _ in 0..30_000 {
        counts[rng.weighted(&[1, 2, 1])] += 1;
    }
    // Weights 1:2:1 -> expected proportions 25%/50%/25% of 30,000: 7,500 /
    // 15,000 / 7,500. Generous bands, since this asserts rough
    // proportionality, not an exact distribution.
    assert!((6_000..9_000).contains(&counts[0]), "counts: {counts:?}");
    assert!((13_000..17_000).contains(&counts[1]), "counts: {counts:?}");
    assert!((6_000..9_000).contains(&counts[2]), "counts: {counts:?}");
}
