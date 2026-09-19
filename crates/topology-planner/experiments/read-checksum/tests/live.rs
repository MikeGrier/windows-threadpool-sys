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
        assert_eq!(capture.trials.len(), 3);
        assert_eq!(capture.file_bytes, bytes);
        assert_eq!(capture.payload_pool_bytes, block * depth);
        for trial in &capture.trials {
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
            for worker in &trial.workers {
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
