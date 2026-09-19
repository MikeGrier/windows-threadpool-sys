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
