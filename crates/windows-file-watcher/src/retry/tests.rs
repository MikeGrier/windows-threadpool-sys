// Copyright (c) 2026 Mike Grier
//! Unit tests for the retry protocol's constants and floor clamp.

use super::*;

#[test]
fn the_floor_clamps_a_shorter_delay() {
    assert_eq!(clamp(Duration::from_millis(1)), FLOOR);
}

#[test]
fn a_delay_at_or_above_the_floor_is_unchanged() {
    assert_eq!(clamp(FLOOR), FLOOR);
    assert_eq!(clamp(Duration::from_secs(1)), Duration::from_secs(1));
}

#[test]
fn open_and_arm_defaults_match_d_27() {
    assert_eq!(
        FaultOperation::Open.default_delay(),
        Duration::from_millis(500)
    );
    assert_eq!(
        FaultOperation::Arm.default_delay(),
        Duration::from_millis(500)
    );
}
