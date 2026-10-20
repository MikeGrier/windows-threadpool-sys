// Copyright (c) Mike Grier.
//! Tests for the pool-growth probe's row.

use super::*;

fn grew(run: GrowthRun, started: usize, threads: usize, arrivals: Vec<u128>) -> GrowthObservation {
    GrowthObservation {
        maximum: run.maximum,
        submitted: run.submissions,
        started_while_blocked: started,
        distinct_threads: threads,
        arrivals_us: arrivals,
    }
}

fn raised(started: usize, took_effect: bool, nanos: u64) -> (RaiseRun, RaiseObservation) {
    (
        RAISE_RUN,
        RaiseObservation {
            base_max: RAISE_RUN.base_max,
            started_before_raise: started,
            delay: Duration::from_nanos(nanos),
            took_effect,
        },
    )
}

fn growth() -> Vec<(GrowthRun, GrowthObservation)> {
    vec![
        (
            GROWTH_RUNS[0],
            grew(GROWTH_RUNS[0], 4, 4, vec![12, 15, 17, 503_001]),
        ),
        (GROWTH_RUNS[1], grew(GROWTH_RUNS[1], 7, 6, vec![3])),
        (GROWTH_RUNS[2], grew(GROWTH_RUNS[2], 0, 0, Vec::new())),
    ]
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_build() {
    let row = parsed(&raw_row(&growth(), raised(2, true, 1), "v1 abc [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-pool-growth");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
}

#[test]
fn each_growth_run_is_published_under_its_name_with_every_arrival() {
    let row = parsed(&raw_row(&growth(), raised(2, true, 1), "b"));
    assert_eq!(
        row["growth"],
        serde_json::json!([
            {"run": "max_4", "maximum": 4, "submitted": 8, "runs_long": false,
             "started_while_blocked": 4, "distinct_threads": 4,
             "arrivals_us": [12, 15, 17, 503_001]},
            {"run": "max_8", "maximum": 8, "submitted": 16, "runs_long": false,
             "started_while_blocked": 7, "distinct_threads": 6,
             "arrivals_us": [3]},
            {"run": "max_4_runs_long", "maximum": 4, "submitted": 8, "runs_long": true,
             "started_while_blocked": 0, "distinct_threads": 0,
             "arrivals_us": []},
        ])
    );
}

#[test]
fn the_raise_is_published_with_its_plan_and_what_happened() {
    let row = parsed(&raw_row(&growth(), raised(2, true, 1_234_567), "b"));
    assert_eq!(
        row["raise"],
        serde_json::json!({
            "base_max": 2, "raised_max": 6, "submitted": 8,
            "started_before_raise": 2, "took_effect": true,
            "elapsed_ns": 1_234_567,
        })
    );
}

#[test]
fn a_raise_that_did_not_take_effect_still_publishes_the_window_it_waited() {
    // Both halves: the boolean says the extra callback never started, and the
    // elapsed time is then the settle window, published as measured rather
    // than hidden behind a null.
    let row = parsed(&raw_row(&growth(), raised(1, false, 2_000_000_123), "b"));
    assert_eq!(row["raise"]["took_effect"], false);
    assert_eq!(row["raise"]["started_before_raise"], 1);
    assert_eq!(row["raise"]["elapsed_ns"], 2_000_000_123_u64);
}

#[test]
fn the_plan_names_every_run_distinctly() {
    let mut names: Vec<&str> = GROWTH_RUNS.iter().map(|run| run.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), GROWTH_RUNS.len());
    for run in GROWTH_RUNS {
        assert_ne!(run.name, run.label, "a record name is not the prose label");
    }
}

#[test]
fn an_arrival_past_u64_saturates_rather_than_wrapping() {
    let run = GROWTH_RUNS[0];
    let row = parsed(&raw_row(
        &[(run, grew(run, 1, 1, vec![u128::from(u64::MAX) + 5]))],
        raised(2, true, 1),
        "b",
    ));
    assert_eq!(
        row["growth"][0]["arrivals_us"],
        serde_json::json!([usize::MAX])
    );
}
