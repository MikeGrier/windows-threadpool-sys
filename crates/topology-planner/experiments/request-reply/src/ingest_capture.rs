// Copyright (c) Mike Grier.
use std::io;
use std::sync::atomic::AtomicBool;
use windows_request_reply_experiment::{Config as Limits, ingest};

pub(super) fn capture() -> io::Result<serde_json::Value> {
    let mut trials = Vec::new();
    for (
        scenario,
        count,
        burst,
        interval,
        transform,
        publish,
        slow,
        fail,
        capacity,
        reorder,
        cancel,
    ) in [
        ("empty", 0, 1, 0, 0, 0, false, 0, 6, 3, None),
        ("single", 1, 1, 0, 2000, 500, false, 0, 6, 3, None),
        ("steady", 64, 1, 100_000, 1000, 200, false, 0, 6, 3, None),
        ("burst", 64, 16, 1_000_000, 1000, 200, false, 0, 6, 3, None),
        ("cheap", 64, 64, 0, 0, 0, false, 0, 6, 3, None),
        (
            "transform-heavy",
            64,
            64,
            0,
            20_000,
            200,
            false,
            0,
            6,
            3,
            None,
        ),
        (
            "publish-heavy",
            64,
            64,
            0,
            200,
            20_000,
            false,
            0,
            6,
            3,
            None,
        ),
        ("slow-first", 64, 64, 0, 4000, 200, true, 0, 6, 3, None),
        ("narrow-window", 64, 64, 0, 4000, 200, true, 0, 6, 1, None),
        ("aborting", 64, 64, 0, 1000, 200, false, 5, 6, 3, None),
        ("pressure", 64, 64, 0, 4000, 1000, true, 0, 3, 1, None),
        (
            "cancellation",
            64,
            64,
            0,
            20_000,
            1000,
            true,
            0,
            6,
            3,
            Some(16),
        ),
    ] {
        let config = ingest::Config {
            limits: Limits {
                workers: 3,
                capacity,
                timeout_ms: 5000,
                cancel_after_admitted: cancel,
            },
            reorder_capacity: reorder,
        };
        let records = ingest::trace(count, burst, interval, transform, publish, slow, fail)?;
        for (position, arrangement) in [
            ingest::Arrangement::SerialOwner,
            ingest::Arrangement::StagedPipeline,
            ingest::Arrangement::StagedPipeline,
            ingest::Arrangement::SerialOwner,
        ]
        .into_iter()
        .enumerate()
        {
            let report = ingest::run(
                config.clone(),
                records.clone(),
                arrangement,
                &AtomicBool::new(false),
            )?;
            let aggregate = ingest::summary(&report, None)?;
            let workers = (0..ingest::transformers(&config, arrangement))
                .map(|worker| ingest::summary(&report, Some(worker)))
                .collect::<io::Result<Vec<_>>>()?;
            trials.push(serde_json::json!({ "scenario": scenario, "position": position, "aggregate": aggregate, "workers": workers, "report": report }));
        }
    }
    Ok(serde_json::Value::Array(trials))
}
