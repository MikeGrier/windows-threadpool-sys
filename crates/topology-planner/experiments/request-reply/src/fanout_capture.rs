// Copyright (c) Mike Grier.
use std::io;
use std::sync::atomic::AtomicBool;
use windows_request_reply_experiment::{Config, fanout};

pub(super) fn capture() -> io::Result<serde_json::Value> {
    let mut trials = Vec::new();
    for (
        scenario,
        count,
        burst,
        interval,
        work,
        units,
        broadcast,
        skew,
        slow,
        fail,
        capacity,
        cancel,
    ) in [
        ("empty", 0, 1, 0, 0, 1, 0, 0, 0, 0, 6, None),
        ("single", 1, 1, 0, 2000, 4, 0, 0, 0, 0, 6, None),
        ("scatter-only", 48, 1, 100_000, 800, 6, 0, 0, 0, 0, 6, None),
        (
            "broadcast-only",
            48,
            1,
            100_000,
            800,
            6,
            1,
            0,
            0,
            0,
            6,
            None,
        ),
        (
            "mixed-shapes",
            48,
            16,
            1_000_000,
            800,
            6,
            2,
            0,
            0,
            0,
            6,
            None,
        ),
        ("wide-fan-out", 48, 48, 0, 400, 16, 3, 0, 0, 0, 6, None),
        ("narrow-fan-out", 48, 48, 0, 400, 2, 2, 0, 0, 0, 6, None),
        ("skewed-width", 48, 48, 0, 400, 12, 3, 2, 0, 0, 6, None),
        ("slow-unit", 48, 48, 0, 1000, 8, 3, 2, 4, 0, 6, None),
        ("aborting", 48, 48, 0, 800, 6, 3, 2, 0, 5, 6, None),
        ("pressure", 48, 48, 0, 2000, 8, 3, 2, 4, 0, 3, None),
        (
            "cancellation",
            48,
            48,
            0,
            20_000,
            8,
            3,
            2,
            4,
            0,
            6,
            Some(12),
        ),
    ] {
        let config = Config {
            workers: 3,
            capacity,
            timeout_ms: 5000,
            cancel_after_admitted: cancel,
        };
        let records = fanout::trace(fanout::TraceSpec {
            count,
            burst,
            interval_ns: interval,
            work,
            units,
            broadcast_every: broadcast,
            skew_every: skew,
            slow_every: slow,
            fail_every: fail,
        })?;
        for (position, arrangement) in [
            fanout::Arrangement::SerialWhole,
            fanout::Arrangement::OwnedJoin,
            fanout::Arrangement::ScatteredJoin,
            fanout::Arrangement::ScatteredJoin,
            fanout::Arrangement::OwnedJoin,
            fanout::Arrangement::SerialWhole,
        ]
        .into_iter()
        .enumerate()
        {
            let report = fanout::run(
                config.clone(),
                records.clone(),
                arrangement,
                &AtomicBool::new(false),
            )?;
            let aggregate = fanout::summary(&report, None)?;
            let scattered = fanout::summary(&report, Some(false))?;
            let broadcast = fanout::summary(&report, Some(true))?;
            trials.push(
                serde_json::json!({ "scenario": scenario, "position": position,
                "aggregate": aggregate, "scatter": scattered, "broadcast": broadcast,
                "report": report }),
            );
        }
    }
    Ok(serde_json::Value::Array(trials))
}
