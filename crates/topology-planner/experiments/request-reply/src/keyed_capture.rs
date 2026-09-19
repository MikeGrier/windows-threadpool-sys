// Copyright (c) Mike Grier.
use std::io;
use std::sync::atomic::AtomicBool;
use windows_request_reply_experiment::{Config as Limits, stateful};

pub(super) fn capture() -> io::Result<serde_json::Value> {
    let mut trials = Vec::new();
    for (scenario, count, keys, burst, interval, hot, writes, work, capacity, cancel_after) in [
        ("empty", 0, 7, 1, 0, false, 50, 0, 6, None),
        ("steady-mixed", 64, 7, 1, 100_000, false, 50, 1000, 6, None),
        ("read-only", 64, 7, 16, 1_000_000, false, 0, 1000, 6, None),
        (
            "write-only",
            64,
            7,
            16,
            1_000_000,
            false,
            100,
            1000,
            6,
            None,
        ),
        (
            "uniform-mixed",
            64,
            7,
            16,
            1_000_000,
            false,
            50,
            1000,
            6,
            None,
        ),
        ("hot-mixed", 64, 7, 16, 1_000_000, true, 50, 1000, 6, None),
        ("all-hot", 64, 1, 64, 0, true, 75, 1000, 6, None),
        ("cheap", 64, 7, 64, 0, false, 50, 0, 6, None),
        ("uneven-work", 64, 7, 64, 0, true, 50, 100_000, 6, None),
        ("pressure", 64, 7, 64, 0, true, 50, 10_000, 3, None),
        ("cancellation", 64, 7, 64, 0, true, 75, 100_000, 6, Some(16)),
    ] {
        let config = stateful::Config {
            limits: Limits {
                workers: 3,
                capacity,
                timeout_ms: 5000,
                cancel_after_admitted: cancel_after,
            },
            keys,
        };
        let requests = stateful::trace(count, keys, burst, interval, hot, writes, work)?;
        for (position, arrangement) in [
            stateful::Arrangement::SharedState,
            stateful::Arrangement::KeyOwned,
            stateful::Arrangement::KeyOwned,
            stateful::Arrangement::SharedState,
        ]
        .into_iter()
        .enumerate()
        {
            let report = stateful::run(
                config.clone(),
                requests.clone(),
                arrangement,
                &AtomicBool::new(false),
            )?;
            let aggregate = stateful::summary(&report, None, None)?;
            let keys = (0..config.keys)
                .map(|key| stateful::summary(&report, Some(key), None))
                .collect::<io::Result<Vec<_>>>()?;
            let lanes = (0..config.limits.workers)
                .map(|lane| stateful::summary(&report, None, Some(lane)))
                .collect::<io::Result<Vec<_>>>()?;
            trials.push(serde_json::json!({ "scenario": scenario, "position": position, "aggregate": aggregate, "keys": keys, "lanes": lanes, "report": report }));
        }
    }
    Ok(serde_json::Value::Array(trials))
}
