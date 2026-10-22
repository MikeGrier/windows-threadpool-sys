// Copyright (c) Mike Grier.

//! The capture scripts refuse to combine runs that different builds produced
//! (`M4.8`), and still accept the runs they were written for.
//!
//! The scripts are JavaScript and nothing else runs them, so this test does,
//! with `node`, against copies of committed runs given a `build:` line. It runs
//! every committed copy of each script, since each capture directory keeps the
//! copy that derived its figures.
//!
//! **It needs `node` on `PATH`**, and fails saying so rather than passing
//! without having run anything.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPTS: &[&str] = &["summarise.js", "isolated.js"];

fn captures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("captures")
}

/// Every capture directory that carries the scripts.
fn script_directories() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(captures())
        .expect("the captures directory exists")
        .map(|entry| entry.expect("a readable entry").path())
        .filter(|dir| SCRIPTS.iter().all(|script| dir.join(script).is_file()))
        .collect();
    found.sort();
    assert!(
        !found.is_empty(),
        "no capture directory carries the scripts"
    );
    found
}

/// A committed run, with `build` inserted after its `profile:` line when given.
fn run_with_build(source: &str, build: Option<&str>) -> String {
    let text =
        std::fs::read_to_string(captures().join("2026-10-19-interleaved-order").join(source))
            .expect("a committed run");
    let Some(build) = build else {
        return text;
    };
    let mut out = String::new();
    for line in text.lines() {
        out.push_str(line);
        out.push('\n');
        if line.starts_with("profile:") {
            out.push_str(build);
            out.push('\n');
        }
    }
    out
}

/// Run `script` over two runs, each with the build line given.
fn combine(directory: &Path, script: &str, first: Option<&str>, second: Option<&str>) -> Output {
    // A counter as well as the process id, because the tests below run in
    // parallel in one process and each calls this many times.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "capture-scripts-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let a = scratch.join("a.txt");
    let b = scratch.join("b.txt");
    std::fs::write(&a, run_with_build("interleaved-run1.txt", first)).expect("written");
    std::fs::write(&b, run_with_build("interleaved-run2.txt", second)).expect("written");

    let output = Command::new("node")
        .arg(directory.join(script))
        .arg(&a)
        .arg(&b)
        .output()
        .unwrap_or_else(|error| {
            panic!("this test runs the capture scripts and needs `node` on PATH: {error}")
        });
    let _ = std::fs::remove_dir_all(&scratch);
    output
}

const ONE: &str = "build: !!UNOFFICIAL!! v2026.1016.0 0123456789ab [LOCAL]";
const OTHER: &str = "build: !!UNOFFICIAL!! v2026.1016.0 ba9876543210 [LOCAL]";

#[test]
fn runs_of_different_builds_are_refused() {
    for directory in script_directories() {
        for script in SCRIPTS {
            for (first, second) in [
                (Some(ONE), Some(OTHER)),
                (Some(ONE), None),
                (None, Some(OTHER)),
            ] {
                let output = combine(&directory, script, first, second);
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(
                    !output.status.success() && stderr.contains("different conditions"),
                    "{}/{script} combined {first:?} with {second:?}: {:?}\n{stderr}",
                    directory.display(),
                    output.status,
                );
            }
        }
    }
}

#[test]
fn runs_of_one_build_are_accepted_whether_stamped_or_not() {
    // The other direction, because a script that refused everything would pass
    // the test above. Unstamped runs are what every committed capture holds, so
    // refusing them would leave no capture regenerable.
    for directory in script_directories() {
        for script in SCRIPTS {
            for build in [Some(ONE), None] {
                let output = combine(&directory, script, build, build);
                assert!(
                    output.status.success(),
                    "{}/{script} refused two runs of {build:?}: {}",
                    directory.display(),
                    String::from_utf8_lossy(&output.stderr),
                );
            }
        }
    }
}
