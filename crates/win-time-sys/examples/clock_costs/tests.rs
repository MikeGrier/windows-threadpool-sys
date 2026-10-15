// Copyright (c) 2026 Mike Grier
//! The probe's arithmetic and its report's shape. What it measures is the machine's, so only how it
//! summarises and reports is tested.

use std::time::Duration;

use super::{Row, Summary, report, smallest_step, summarize};

#[test]
fn a_summary_is_the_least_the_lower_middle_and_the_most() {
    let mut odd = [5.0, 1.0, 3.0];
    assert_eq!(
        summarize(&mut odd),
        Some(Summary {
            least: 1.0,
            median: 3.0,
            most: 5.0
        })
    );
    let mut even = [4.0, 2.0, 8.0, 6.0];
    assert_eq!(
        summarize(&mut even),
        Some(Summary {
            least: 2.0,
            median: 4.0,
            most: 8.0
        })
    );
    let mut one = [7.5];
    assert_eq!(
        summarize(&mut one),
        Some(Summary {
            least: 7.5,
            median: 7.5,
            most: 7.5
        })
    );
    assert_eq!(summarize(&mut []), None);
}

#[test]
fn the_smallest_step_ignores_repeats_and_backward_moves() {
    assert_eq!(smallest_step(&[100, 300, 350, 600]), Some(50));
    assert_eq!(smallest_step(&[100, 100, 200, 200, 210]), Some(10));
    assert_eq!(
        smallest_step(&[500, 400, 450]),
        Some(50),
        "a backward move is not a step"
    );
    assert_eq!(smallest_step(&[7, 7, 7]), None, "a clock that never moved");
    assert_eq!(smallest_step(&[7]), None);
    assert_eq!(smallest_step(&[]), None);
}

#[test]
fn the_report_names_every_clock_and_what_was_seen() {
    let rows = [
        Row {
            name: "Moving",
            cost: Summary {
                least: 1.25,
                median: 2.5,
                most: 9.0,
            },
            step: Some(Duration::from_nanos(100)),
        },
        Row {
            name: "Still",
            cost: Summary {
                least: 3.0,
                median: 3.0,
                most: 3.0,
            },
            step: None,
        },
    ];
    let mut text = String::new();
    report(&mut text, 10_000_000, &rows).expect("a String");
    assert!(text.contains("10000000 Hz"), "{text}");
    let moving = text
        .lines()
        .find(|line| line.starts_with("Moving"))
        .expect("a row");
    assert!(
        moving.contains("1.2") && moving.contains("2.5") && moving.contains("9.0"),
        "{moving}"
    );
    assert!(moving.contains("100ns"), "{moving}");
    let still = text
        .lines()
        .find(|line| line.starts_with("Still"))
        .expect("a row");
    assert!(still.contains("none seen"), "{still}");
}
