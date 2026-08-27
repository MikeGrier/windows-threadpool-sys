// Copyright (c) 2026 Mike Grier
//! Tests for the native timestamp newtype.

use super::*;

#[test]
fn ticks_round_trip() {
    let timestamp = WindowsFileTimestamp::from_ticks(133_000_000_000_000_000);
    assert_eq!(timestamp.ticks(), 133_000_000_000_000_000);
}

#[test]
fn zero_is_preserved_rather_than_reinterpreted() {
    // A filesystem that does not track a time reports zero, and that value must
    // reach a caller unchanged rather than becoming "unknown".
    assert_eq!(WindowsFileTimestamp::ZERO.ticks(), 0);
    assert_eq!(WindowsFileTimestamp::default(), WindowsFileTimestamp::ZERO);
}

#[test]
fn negative_ticks_are_preserved() {
    let timestamp = WindowsFileTimestamp::from_ticks(-1);
    assert_eq!(timestamp.ticks(), -1);
}

#[test]
fn ordering_follows_the_raw_tick_count() {
    let earlier = WindowsFileTimestamp::from_ticks(10);
    let later = WindowsFileTimestamp::from_ticks(20);
    assert!(earlier < later);
    assert!(WindowsFileTimestamp::from_ticks(-5) < WindowsFileTimestamp::ZERO);
}

#[test]
fn filetime_round_trips_through_both_words() {
    let timestamp = WindowsFileTimestamp::from_ticks(0x1234_5678_9ABC_DEF0);
    let filetime = timestamp.to_filetime();
    assert_eq!(filetime.dwLowDateTime, 0x9ABC_DEF0);
    assert_eq!(filetime.dwHighDateTime, 0x1234_5678);
    assert_eq!(WindowsFileTimestamp::from_filetime(filetime), timestamp);
}

#[test]
fn filetime_round_trips_a_negative_value() {
    let timestamp = WindowsFileTimestamp::from_ticks(-2);
    assert_eq!(
        WindowsFileTimestamp::from_filetime(timestamp.to_filetime()),
        timestamp
    );
}

#[test]
fn filetime_round_trips_zero_and_extremes() {
    for ticks in [0, 1, -1, i64::MAX, i64::MIN] {
        let timestamp = WindowsFileTimestamp::from_ticks(ticks);
        assert_eq!(
            WindowsFileTimestamp::from_filetime(timestamp.to_filetime()),
            timestamp,
            "round trip failed for {ticks}"
        );
    }
}

#[test]
fn conversions_to_and_from_i64_agree_with_the_accessors() {
    let timestamp = WindowsFileTimestamp::from(99_i64);
    assert_eq!(timestamp.ticks(), 99);
    assert_eq!(i64::from(timestamp), 99);
}

#[test]
fn display_names_the_unit() {
    assert_eq!(WindowsFileTimestamp::from_ticks(7).to_string(), "7 ticks");
}
