// Copyright (c) Mike Grier.
#![cfg(windows)]

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows_read_checksum_experiment::{Arrangement, Config, run, write_fixture};
use windows_topology_sys::ProcessorId;

struct Fixture(PathBuf);

impl Fixture {
    fn new(bytes: u64) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(4)
            .unwrap()
            .join(".scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let path = scratch.join(format!(
            "read-checksum-test-{}-{}.dat",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        write_fixture(&mut file, bytes).unwrap();
        Self(path)
    }

    fn config(&self, block_bytes: usize, depth: usize, queue_capacity: usize) -> Config {
        Config {
            file: self.0.clone(),
            block_bytes,
            depth,
            buffer_count: None,
            batch_size: 1,
            queue_capacity,
            checksum_passes: 1,
            repetitions: 1,
            timeout_ms: 10_000,
            processors: None,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).expect("remove this test's fixture");
    }
}

#[test]
fn all_arrangements_process_ten_shapes_with_equal_payload_budgets() {
    for (block, blocks, tail, depth, queue, passes) in [
        (1, 2, 0, 2, 1, 1),
        (7, 7, 1, 2, 1, 2),
        (128, 3, 127, 8, 4, 1),
        (512, 8, 0, 4, 2, 2),
        (1024, 9, 0, 4, 1, 1),
        (4096, 20, 13, 8, 1, 4),
        (4096, 16, 0, 8, 8, 1),
        (8192, 32, 3, 16, 2, 2),
        (16384, 10, 0, 4, 4, 1),
        (65536, 4, 11, 8, 1, 2),
    ] {
        let bytes = (block * blocks + tail) as u64;
        let fixture = Fixture::new(bytes);
        let mut config = fixture.config(block, depth, queue);
        config.checksum_passes = passes;
        let capture = run(config).unwrap();
        assert_eq!(capture.trials.len(), 6);
        assert_eq!(capture.schema, "read-checksum-v3");
        assert_eq!(capture.file_bytes, bytes);
        assert_eq!(capture.payload_pool_bytes, block * depth);
        for trial in &capture.trials {
            let (workers, checksum_workers) = match trial.arrangement {
                Arrangement::Direct => (1, 1),
                Arrangement::Pipeline => (2, 1),
                Arrangement::Independent => (2, 2),
            };
            assert_eq!(trial.workers.len(), workers);
            assert_eq!(trial.resources.worker_threads, workers);
            assert_eq!(trial.resources.participating_processors, workers);
            assert_eq!(trial.resources.checksum_workers, checksum_workers);
            assert_eq!(trial.resources.unequal_cpu_reference, workers == 1);
            assert_eq!(trial.resources.max_pending_reads, depth);
            assert_eq!(trial.resources.payload_pool_bytes, block * depth);
            assert_eq!(
                trial
                    .workers
                    .iter()
                    .map(|worker| worker.buffer_capacity)
                    .sum::<usize>(),
                depth
            );
            assert_eq!(
                trial
                    .workers
                    .iter()
                    .map(|worker| worker.checksummed_blocks)
                    .sum::<usize>(),
                trial.completed_blocks
            );
            assert_eq!(
                trial
                    .workers
                    .iter()
                    .filter(|worker| worker.checksummed_blocks > 0)
                    .count(),
                checksum_workers
            );
            assert_eq!(trial.completed_blocks, blocks + usize::from(tail != 0));
            assert_eq!(trial.checksum_latency.count, trial.completed_blocks);
            assert!(trial.work_wall_ns > 0);
            assert!(trial.joined_wall_ns >= trial.work_wall_ns);
            assert!(trial.bytes_per_second.is_finite());
            assert_eq!(
                trial.workers.iter().map(|w| w.submitted).sum::<usize>(),
                trial.completed_blocks
            );
            assert!(
                trial
                    .workers
                    .iter()
                    .map(|w| w.peak_leased_buffers)
                    .sum::<usize>()
                    <= depth
            );
            for (index, worker) in trial.workers.iter().enumerate() {
                let expected_index = if trial.reversed { 1 - index } else { index };
                assert_eq!(
                    worker.requested_processor,
                    capture.processors[expected_index]
                );
                assert_eq!(worker.processor_at_start, worker.requested_processor);
                assert_eq!(worker.processor_at_end, worker.requested_processor);
                if worker.role != "processor" {
                    let before = worker.pages_before.as_ref().unwrap();
                    assert!(
                        before.pages_by_node.values().sum::<usize>() + before.unresident_pages > 0
                    );
                    assert!(worker.pages_after.is_some());
                }
            }
            assert_eq!(
                trial.handoff_latency.is_some(),
                trial.arrangement == Arrangement::Pipeline
            );
        }
        for arrangement in [
            Arrangement::Direct,
            Arrangement::Pipeline,
            Arrangement::Independent,
        ] {
            for reversed in [false, true] {
                assert_eq!(
                    capture
                        .trials
                        .iter()
                        .filter(
                            |trial| trial.arrangement == arrangement && trial.reversed == reversed
                        )
                        .count(),
                    1
                );
            }
        }
    }
}

#[test]
fn invalid_inputs_are_errors_not_empty_captures() {
    for bytes in [0, 1, 4096] {
        let fixture = Fixture::new(bytes);
        assert!(run(fixture.config(4096, 2, 1)).is_err());
    }
    let fixture = Fixture::new(8192);
    let mut config = fixture.config(4096, 2, 1);
    config.file = fixture.0.with_extension("missing");
    assert_eq!(run(config).unwrap_err().kind(), io::ErrorKind::NotFound);
    let mut config = fixture.config(4096, 2, 1);
    config.processors = Some([
        ProcessorId {
            group: u16::MAX,
            number: 0,
        },
        ProcessorId {
            group: u16::MAX,
            number: 1,
        },
    ]);
    assert!(run(config).is_err());
    let mut config = fixture.config(4096, 2, 1);
    config.processors = Some(
        [ProcessorId {
            group: 0,
            number: 0,
        }; 2],
    );
    assert!(run(config).is_err());
}

#[test]
fn recorded_schedule_uses_each_balanced_row() {
    let fixture = Fixture::new(8193);
    let mut config = fixture.config(1024, 8, 4);
    config.repetitions = 6;
    let capture = run(config).unwrap();
    assert_eq!(capture.trials.len(), 36);
    for (repetition, row) in capture.trials.as_chunks::<6>().0.iter().enumerate() {
        let expected = windows_read_checksum_experiment::comparison_order(repetition);
        for (position, (trial, comparison)) in row.iter().zip(expected).enumerate() {
            assert_eq!(trial.repetition, repetition);
            assert_eq!(trial.position, position);
            assert_eq!(trial.arrangement, comparison.arrangement);
            assert_eq!(trial.reversed, comparison.reversed);
        }
    }
}

#[test]
fn separate_read_credits_and_batches_preserve_tails_and_budgets() {
    for (depth, buffers, batch, queue) in [
        (2, 8, 1, 1),
        (2, 8, 3, 1),
        (2, 8, 16, 8),
        (4, 16, 4, 1),
        (4, 16, 16, 4),
        (8, 32, 3, 4),
        (8, 32, 16, 1),
        (8, 32, 64, 32),
        (16, 32, 16, 1),
        (32, 32, 1024, 16),
    ] {
        let fixture = Fixture::new(4096 * 33 + 17);
        let mut config = fixture.config(4096, depth, queue);
        config.buffer_count = Some(buffers);
        config.batch_size = batch;
        let capture = run(config).unwrap();
        assert_eq!(capture.blocks, 34);
        for trial in capture.trials {
            assert_eq!(trial.completed_blocks, capture.blocks);
            assert_eq!(trial.resources.max_pending_reads, depth);
            assert_eq!(trial.resources.payload_pool_bytes, buffers * 4096);
            assert_eq!(
                trial
                    .workers
                    .iter()
                    .map(|worker| worker.read_capacity)
                    .sum::<usize>(),
                depth
            );
            assert_eq!(
                trial
                    .workers
                    .iter()
                    .map(|worker| worker.buffer_capacity)
                    .sum::<usize>(),
                buffers
            );
            for worker in trial.workers {
                assert!(worker.peak_outstanding_reads <= worker.read_capacity);
                assert!(worker.peak_leased_buffers <= worker.buffer_capacity);
                assert_eq!(worker.batches.jobs, worker.completed);
                assert!(worker.batches.max_jobs > 0 && worker.batches.max_jobs <= batch);
                assert!(worker.batches.count <= worker.batches.jobs);
                if worker.completed % batch != 0 {
                    assert!(worker.batches.partial > 0);
                }
                if worker.role == "reader" {
                    assert_eq!(worker.return_batches.jobs, worker.completed);
                    assert!(worker.return_batches.max_jobs <= batch);
                    assert!(worker.handoff_high_water.unwrap() <= queue);
                }
                if worker.role == "processor" {
                    assert!(worker.return_high_water.unwrap() <= buffers);
                }
            }
        }
    }
}

#[test]
fn incompatible_file_share_is_an_explicit_error() {
    let fixture = Fixture::new(8192);
    let _exclusive = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&fixture.0)
        .unwrap();
    assert!(run(fixture.config(4096, 2, 1)).is_err());
}

#[test]
fn sweep_plan_is_bracketed_reversed_and_changes_only_declared_fields() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let destination = root
        .ancestors()
        .nth(4)
        .unwrap()
        .join(".scratch")
        .join(format!("plan-only-{}", std::process::id()));
    let output = std::process::Command::new("pwsh")
        .args(["-NoProfile", "-File"])
        .arg(root.join("capture-ep-x1-1.ps1"))
        .args(["-Study", "EP-X1.2", "-PlanOnly", "-OutputDirectory"])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!destination.exists());
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let cases = plan["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 38);
    let baseline = &cases[0]["Config"];
    for sweep in ["block", "compute", "depth", "queue", "batch", "queue-batch"] {
        let rows: Vec<_> = cases.iter().filter(|case| case["Sweep"] == sweep).collect();
        assert_eq!(rows.first().unwrap()["Point"], "control");
        assert_eq!(rows.last().unwrap()["Point"], "control");
        assert_eq!(rows.len(), if sweep == "queue-batch" { 8 } else { 6 });
        for (forward, backward) in rows.iter().zip(rows.iter().rev()) {
            assert_eq!(forward["Config"], backward["Config"]);
            let config: Config = serde_json::from_value(forward["Config"].clone()).unwrap();
            assert!(
                config
                    .validate(plan["fixture_bytes"].as_u64().unwrap())
                    .is_ok()
            );
            assert_eq!(config.block_bytes * config.buffers(), 2 * 1024 * 1024);
            let changed: Vec<_> = forward["Config"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, value)| **value != baseline[*key])
                .map(|(key, _)| key.as_str())
                .collect();
            let declared: Vec<_> = forward["ChangedFields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| field.as_str().unwrap())
                .collect();
            assert_eq!(changed.len(), declared.len());
            assert!(changed.iter().all(|field| declared.contains(field)));
            assert!(
                changed.len()
                    <= if sweep == "block" || sweep == "queue-batch" {
                        2
                    } else {
                        1
                    }
            );
        }
    }
}

#[test]
fn reference_computation_obeys_cooperative_deadline() {
    let fixture = Fixture::new(2 * 1024 * 1024);
    let mut config = fixture.config(1024 * 1024, 2, 1);
    config.timeout_ms = 1;
    config.checksum_passes = 1024;
    let start = Instant::now();
    assert_eq!(run(config).unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn summary_replay_keeps_sweeps_and_points_separate() {
    let fixture = Fixture::new(8193);
    let capture = run(fixture.config(1024, 8, 4)).unwrap();
    let directory = fixture.0.with_extension("summary");
    std::fs::create_dir(&directory).unwrap();
    let mut cases = Vec::new();
    for (sweep, base, altered) in [
        ("first-sweep", 100.0, 200.0),
        ("second-sweep", 1000.0, 1000.0),
    ] {
        for (position, point) in ["control", "changed", "control"].into_iter().enumerate() {
            let name = format!("{sweep}-{position}");
            let mut report = serde_json::to_value(&capture).unwrap();
            for trial in report["trials"].as_array_mut().unwrap() {
                trial["bytes_per_second"] =
                    serde_json::json!(if point == "control" { base } else { altered });
            }
            serde_json::to_writer(
                File::create(directory.join(format!("{name}.json"))).unwrap(),
                &report,
            )
            .unwrap();
            cases.push(serde_json::json!({"Name": name, "Sweep": sweep, "Point": point, "ChangedFields": [], "Config": capture.config}));
        }
    }
    serde_json::to_writer(
        File::create(directory.join("matrix.json")).unwrap(),
        &serde_json::json!({"study": "EP-X1.2", "cases": cases}),
    )
    .unwrap();
    let old_summary =
        br#"{"executable_sha256":"fixture-binary","script_sha256":"fixture-capture-script"}"#;
    std::fs::write(directory.join("summary.json"), old_summary).unwrap();
    let output = std::process::Command::new("pwsh")
        .args(["-NoProfile", "-File"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("capture-ep-x1-1.ps1"))
        .args(["-SummarizeOnly", "-OutputDirectory"])
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["capture_script_sha256"], "fixture-capture-script");
    assert_eq!(
        std::fs::read(directory.join("summary.json")).unwrap(),
        old_summary
    );
    let effects = summary["effects"].as_array().unwrap();
    assert_eq!(effects.len(), 12);
    for effect in effects {
        let expected = match effect["sweep"].as_str().unwrap() {
            "first-sweep" => (100.0, 200.0),
            "second-sweep" => (1000.0, 1000.0),
            other => panic!("unexpected sweep {other}"),
        };
        assert_eq!(effect["point"], "changed");
        assert_eq!(effect["control_bytes_per_second"]["min"], expected.0);
        assert_eq!(effect["point_bytes_per_second"]["max"], expected.1);
        assert_eq!(effect["ranges_overlap"], expected.0 == expected.1);
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cli_refuses_to_overwrite_fixture() {
    let fixture = Fixture::new(8192);
    let binary = env!("CARGO_BIN_EXE_windows-read-checksum-experiment");
    let output = std::process::Command::new(binary)
        .args(["fixture"])
        .arg(&fixture.0)
        .arg("100")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        File::open(&fixture.0).unwrap().metadata().unwrap().len(),
        8192
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["status"], "error");
}

#[test]
fn cli_persists_failure_and_refuses_existing_report() {
    let fixture = Fixture::new(1);
    let config_file = Fixture::new(0);
    serde_json::to_writer(
        File::create(&config_file.0).unwrap(),
        &fixture.config(4096, 2, 1),
    )
    .unwrap();
    let report_path = config_file.0.with_extension("report.json");
    let binary = env!("CARGO_BIN_EXE_windows-read-checksum-experiment");
    let run_cli = || {
        std::process::Command::new(binary)
            .arg("run")
            .arg(&config_file.0)
            .arg(&report_path)
            .output()
            .unwrap()
    };
    let output = run_cli();
    let report = Fixture(report_path.clone());
    assert!(!output.status.success());
    let contents = std::fs::read(&report.0).unwrap();
    let status: serde_json::Value = serde_json::from_slice(&contents).unwrap();
    assert_eq!(status["status"], "error");
    let output = run_cli();
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&report.0).unwrap(), contents);
}
