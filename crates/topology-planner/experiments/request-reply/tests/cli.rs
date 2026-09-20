// Copyright (c) Mike Grier.
#![cfg(windows)]

use std::path::Path;
use std::process::Command;
use windows_request_reply_experiment::{Report, verify};

#[test]
fn capture_cli_preserves_verified_trials_and_refuses_overwrite() {
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap()
        .join(".scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let path = scratch.join(format!("request-reply-cli-{}.json", std::process::id()));
    let binary = env!("CARGO_BIN_EXE_windows-request-reply-experiment");
    let invoke = || {
        Command::new(binary)
            .arg("capture")
            .arg(&path)
            .output()
            .unwrap()
    };
    let output = invoke();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let bytes = std::fs::read(&path).unwrap();
    let capture: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(capture["status"], "success");
    let trials = capture["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 24);
    for group in trials.as_chunks::<4>().0 {
        assert_eq!(group[0]["report"]["arrangement"], "shared");
        assert_eq!(group[1]["report"]["arrangement"], "assigned");
        assert_eq!(group[2]["report"]["arrangement"], "assigned");
        assert_eq!(group[3]["report"]["arrangement"], "shared");
        for trial in group {
            let report: Report = serde_json::from_value(trial["report"].clone()).unwrap();
            verify(&report).unwrap();
            assert_eq!(trial["report"]["trace"], group[0]["report"]["trace"]);
            assert_eq!(trial["report"]["config"], group[0]["report"]["config"]);
        }
    }
    assert!(!invoke().status.success());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
    let output = Command::new(binary).arg("unknown").output().unwrap();
    assert!(!output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["status"],
        "error"
    );
}

#[test]
fn stateful_capture_preserves_both_candidates_state_and_per_key_summaries() {
    use windows_request_reply_experiment::{StopReason, stateful};
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap()
        .join(".scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let path = scratch.join(format!("stateful-cli-{}.json", std::process::id()));
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_windows-request-reply-experiment"))
            .arg("capture-stateful")
            .arg(&path)
            .output()
            .unwrap()
    };
    let output = invoke();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let bytes = std::fs::read(&path).unwrap();
    let capture: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(capture["schema"], "stateful-capture-v1");
    let trials = capture["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 44);
    for group in trials.as_chunks::<4>().0 {
        let mut first_state = None;
        for (position, trial) in group.iter().enumerate() {
            let report: stateful::Report = serde_json::from_value(trial["report"].clone()).unwrap();
            stateful::verify(&report).unwrap();
            assert_eq!(
                report.arrangement,
                if position == 0 || position == 3 {
                    stateful::Arrangement::SharedState
                } else {
                    stateful::Arrangement::KeyOwned
                }
            );
            assert_eq!(trial["report"]["trace"], group[0]["report"]["trace"]);
            assert_eq!(trial["report"]["config"], group[0]["report"]["config"]);
            assert_eq!(trial["keys"].as_array().unwrap().len(), report.config.keys);
            for key in 0..report.config.keys {
                let expected =
                    serde_json::to_value(stateful::summary(&report, Some(key), None).unwrap())
                        .unwrap();
                assert_eq!(trial["keys"][key], expected);
            }
            if trial["scenario"] != "cancellation" {
                assert_eq!(report.stop, StopReason::Drained);
                if let Some(state) = &first_state {
                    assert_eq!(&report.final_state, state);
                } else {
                    first_state = Some(report.final_state);
                }
            } else {
                assert_eq!(report.stop, StopReason::Cancelled);
                assert_eq!(report.replies.len(), 16);
            }
        }
    }
    assert!(!invoke().status.success());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn fanout_capture_preserves_membership_and_declared_join_order() {
    use windows_request_reply_experiment::{StopReason, fanout};
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap()
        .join(".scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let path = scratch.join(format!("fanout-cli-{}.json", std::process::id()));
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_windows-request-reply-experiment"))
            .arg("capture-fanout")
            .arg(&path)
            .output()
            .unwrap()
    };
    let output = invoke();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let bytes = std::fs::read(&path).unwrap();
    let capture: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(capture["schema"], "fanout-capture-v1");
    let trials = capture["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 72);
    let expected = [
        fanout::Arrangement::SerialWhole,
        fanout::Arrangement::OwnedJoin,
        fanout::Arrangement::ScatteredJoin,
        fanout::Arrangement::ScatteredJoin,
        fanout::Arrangement::OwnedJoin,
        fanout::Arrangement::SerialWhole,
    ];
    for group in trials.as_chunks::<6>().0 {
        for (position, trial) in group.iter().enumerate() {
            let report: fanout::Report = serde_json::from_value(trial["report"].clone()).unwrap();
            fanout::verify(&report).unwrap();
            assert_eq!(report.arrangement, expected[position]);
            assert_eq!(trial["report"]["trace"], group[0]["report"]["trace"]);
            assert_eq!(trial["report"]["config"], group[0]["report"]["config"]);
            assert_eq!(report.residual_partial_units, 0);
            if trial["scenario"] == "cancellation" {
                assert_eq!(report.stop, StopReason::Cancelled);
            } else {
                assert_eq!(report.stop, StopReason::Drained);
                // Fan-out must not change which records join, or in what order.
                assert_eq!(trial["report"]["log"], group[0]["report"]["log"]);
            }
        }
    }
    assert!(!invoke().status.success());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn ingest_capture_preserves_declared_publication_order_across_both_arrangements() {
    use windows_request_reply_experiment::{StopReason, ingest};
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap()
        .join(".scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let path = scratch.join(format!("ingest-cli-{}.json", std::process::id()));
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_windows-request-reply-experiment"))
            .arg("capture-ingest")
            .arg(&path)
            .output()
            .unwrap()
    };
    let output = invoke();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let bytes = std::fs::read(&path).unwrap();
    let capture: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(capture["schema"], "ingest-capture-v1");
    let trials = capture["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 48);
    for group in trials.as_chunks::<4>().0 {
        for (position, trial) in group.iter().enumerate() {
            let report: ingest::Report = serde_json::from_value(trial["report"].clone()).unwrap();
            ingest::verify(&report).unwrap();
            assert_eq!(
                report.arrangement,
                if position == 0 || position == 3 {
                    ingest::Arrangement::SerialOwner
                } else {
                    ingest::Arrangement::StagedPipeline
                }
            );
            assert_eq!(trial["report"]["trace"], group[0]["report"]["trace"]);
            assert_eq!(trial["report"]["config"], group[0]["report"]["config"]);
            assert_eq!(
                trial["workers"].as_array().unwrap().len(),
                ingest::transformers(&report.config, report.arrangement)
            );
            let expected = serde_json::to_value(ingest::summary(&report, None).unwrap()).unwrap();
            assert_eq!(trial["aggregate"], expected);
            if trial["scenario"] == "cancellation" {
                assert_eq!(report.stop, StopReason::Cancelled);
            } else {
                assert_eq!(report.stop, StopReason::Drained);
                // Declared order is the contract, so every position must publish identically.
                assert_eq!(trial["report"]["log"], group[0]["report"]["log"]);
            }
        }
    }
    assert!(!invoke().status.success());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
}
