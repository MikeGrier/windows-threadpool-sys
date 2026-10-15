// Copyright (c) 2026 Mike Grier
//! The two layers' arithmetic, over fake timelines whose periods exercise exact, inexact and
//! sub-nanosecond ticks.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::time::Duration;

use super::{Ticks, TimePoint, Timeline};

/// 100 ns ticks, the period of system and interrupt time.
enum Hundreds {}
impl Timeline for Hundreds {
    fn ticks_per_second() -> NonZeroU64 {
        NonZeroU64::new(10_000_000).unwrap()
    }
}

/// A period that divides no power of ten.
enum Thirds {}
impl Timeline for Thirds {
    fn ticks_per_second() -> NonZeroU64 {
        NonZeroU64::new(3).unwrap()
    }
}

/// Ticks shorter than a nanosecond.
enum Fast {}
impl Timeline for Fast {
    fn ticks_per_second() -> NonZeroU64 {
        NonZeroU64::new(3_000_000_000).unwrap()
    }
}

type P = TimePoint<Hundreds>;
type T = Ticks<Hundreds>;

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn a_point_is_recorded_and_read_back_as_its_tick_count() {
    for ticks in [0, 1, 133_000_000_000_000_000, u64::MAX] {
        assert_eq!(P::from_ticks(ticks).ticks(), ticks);
        assert_eq!(P::from_ticks(ticks), P::from_ticks(ticks));
    }
}

#[test]
fn points_order_and_hash_by_their_ticks() {
    let (early, late) = (P::from_ticks(5), P::from_ticks(9));
    assert!(early < late && late > early && early != late);
    assert_eq!(early.max(late), late);
    assert_eq!(hash_of(early), hash_of(P::from_ticks(5)));
    let mut points = [late, early, P::from_ticks(7)];
    points.sort();
    assert_eq!(points.map(TimePoint::ticks), [5, 7, 9]);
}

#[test]
fn subtracting_points_gives_a_signed_distance() {
    let (early, late) = (P::from_ticks(100), P::from_ticks(350));
    assert_eq!(late - early, T::new(250));
    assert_eq!(early - late, T::new(-250));
    assert_eq!(early - early, T::ZERO);
    assert!((early - late).is_negative() && !(late - early).is_negative());
}

#[test]
fn a_distance_that_does_not_fit_is_refused() {
    let edge = 1_u64 << 63;
    assert_eq!(
        P::from_ticks(edge - 1).checked_since(P::from_ticks(0)),
        Some(T::new(i64::MAX))
    );
    assert_eq!(
        P::from_ticks(0).checked_since(P::from_ticks(edge)),
        Some(T::new(i64::MIN))
    );
    assert_eq!(
        P::from_ticks(edge).checked_since(P::from_ticks(0)),
        None,
        "one past i64::MAX"
    );
    assert_eq!(
        P::from_ticks(0).checked_since(P::from_ticks(edge + 1)),
        None,
        "one past i64::MIN"
    );
    assert_eq!(
        P::from_ticks(u64::MAX).checked_since(P::from_ticks(0)),
        None
    );
}

#[test]
fn moving_a_point_stays_on_the_timeline_or_is_refused() {
    let point = P::from_ticks(1_000);
    assert_eq!(point + T::new(24), P::from_ticks(1_024));
    assert_eq!(point - T::new(24), P::from_ticks(976));
    assert_eq!(point + T::new(-1_000), P::from_ticks(0));
    assert_eq!(point.checked_add(T::new(-1_001)), None, "before zero");
    assert_eq!(point.checked_sub(T::new(1_001)), None, "before zero");
    let last = P::from_ticks(u64::MAX);
    assert_eq!(last.checked_add(T::new(1)), None, "past the end");
    assert_eq!(last.checked_sub(T::new(-1)), None, "past the end");
    assert_eq!(
        P::from_ticks(0).checked_sub(T::new(i64::MIN)),
        Some(P::from_ticks(1 << 63)),
        "the one distance with no opposite still moves a point"
    );
    let mut moved = point;
    moved += T::new(10);
    moved -= T::new(4);
    assert_eq!(moved, P::from_ticks(1_006));
}

#[test]
fn a_distance_becomes_a_duration_when_it_points_forwards() {
    assert_eq!(
        T::new(25_000_000).to_duration(),
        Some(Duration::from_millis(2_500))
    );
    assert_eq!(T::new(1).to_duration(), Some(Duration::from_nanos(100)));
    assert_eq!(T::ZERO.to_duration(), Some(Duration::ZERO));
    assert_eq!(T::new(-1).to_duration(), None);
    assert_eq!(
        T::new(-25_000_000).abs_duration(),
        Duration::from_millis(2_500)
    );
}

#[test]
fn an_inexact_or_sub_nanosecond_tick_truncates_towards_zero() {
    assert_eq!(
        Ticks::<Thirds>::new(4).to_duration(),
        Some(Duration::new(1, 333_333_333))
    );
    assert_eq!(
        Ticks::<Thirds>::new(-4).abs_duration(),
        Duration::new(1, 333_333_333)
    );
    assert_eq!(Ticks::<Fast>::new(2).to_duration(), Some(Duration::ZERO));
    assert_eq!(
        Ticks::<Fast>::new(3).to_duration(),
        Some(Duration::from_nanos(1))
    );
    assert_eq!(
        Ticks::<Fast>::new(3_000_000_007).to_duration(),
        Some(Duration::new(1, 2))
    );
}

#[test]
fn the_extremes_convert_without_overflowing() {
    let longest = T::new(i64::MAX).to_duration().expect("forwards");
    assert_eq!(longest.as_secs(), i64::MAX.unsigned_abs() / 10_000_000);
    assert_eq!(
        T::new(i64::MIN).abs_duration(),
        Duration::new(
            i64::MIN.unsigned_abs() / 10_000_000,
            u32::try_from(i64::MIN.unsigned_abs() % 10_000_000 * 100).unwrap()
        )
    );
    assert_eq!(T::from_duration(longest), Some(T::new(i64::MAX)));
}

#[test]
fn a_duration_becomes_ticks_truncated_towards_zero() {
    assert_eq!(
        T::from_duration(Duration::from_millis(2_500)),
        Some(T::new(25_000_000))
    );
    assert_eq!(T::from_duration(Duration::from_nanos(199)), Some(T::new(1)));
    assert_eq!(T::from_duration(Duration::ZERO), Some(T::ZERO));
    assert_eq!(
        Ticks::<Thirds>::from_duration(Duration::from_millis(500)),
        Some(Ticks::new(1))
    );
    assert_eq!(
        Ticks::<Thirds>::from_duration(Duration::from_secs(1)),
        Some(Ticks::new(3))
    );
    assert_eq!(
        Ticks::<Fast>::from_duration(Duration::from_nanos(1)),
        Some(Ticks::new(3))
    );
    assert_eq!(
        T::from_duration(Duration::MAX),
        None,
        "too long for i64 ticks"
    );
}

#[test]
fn a_whole_number_of_ticks_survives_a_round_trip_through_a_duration() {
    for count in [0, 1, 7, 9_999_999, 10_000_000, 123_456_789_012] {
        let ticks = T::new(count);
        assert_eq!(T::from_duration(ticks.to_duration().unwrap()), Some(ticks));
    }
}

#[test]
fn distances_add_subtract_and_negate_or_refuse_to_overflow() {
    assert_eq!(T::new(5) + T::new(-8), T::new(-3));
    assert_eq!(T::new(5) - T::new(8), T::new(-3));
    assert_eq!(-T::new(5), T::new(-5));
    let mut total = T::default();
    total += T::new(4);
    total -= T::new(1);
    assert_eq!(total, T::new(3));
    assert_eq!(T::new(i64::MAX).checked_add(T::new(1)), None);
    assert_eq!(T::new(i64::MIN).checked_sub(T::new(1)), None);
    assert_eq!(T::new(i64::MIN).checked_neg(), None);
    assert!(T::new(-1) < T::ZERO && T::new(2).max(T::new(3)) == T::new(3));
    assert_eq!(hash_of(T::new(3)), hash_of(total));
}

#[test]
#[should_panic(expected = "the distance between two time points overflowed")]
fn subtracting_points_too_far_apart_panics() {
    let _ = P::from_ticks(u64::MAX) - P::from_ticks(0);
}

#[test]
#[should_panic(expected = "a time point moved outside its timeline")]
fn moving_a_point_off_the_timeline_panics() {
    let _ = P::from_ticks(0) - T::new(1);
}

#[test]
#[should_panic(expected = "a tick count overflowed")]
fn an_overflowing_distance_panics() {
    let _ = T::new(i64::MAX) + T::new(1);
}

#[test]
fn debug_names_the_timeline() {
    let shown = format!("{:?} {:?}", P::from_ticks(3), T::new(-2));
    assert!(
        shown.contains("Hundreds") && shown.contains("ticks: 3") && shown.contains("count: -2"),
        "{shown}"
    );
}
