// Copyright (c) Mike Grier.

//! Which probes carry a machine-readable row, established by running them.
//!
//! **This replaces a unit test that asked the wrong question and got the right
//! answer.** That test walked `src/bin` and grepped each file for the string
//! `x-probe`. It was wrong twice: the walk was shallow, so it never saw
//! `queue_contention`, whose source is `src/bin/queue_contention/main.rs` -- and
//! `queue_contention` is precisely the probe whose classification the test was
//! written to stop a document getting wrong. And a bare substring matched
//! `topology.rs`, whose only mention of `x-probe` is a `//!` comment; its row is
//! emitted from `topology_report`, in the library. The asserted set happened to
//! be correct while neither half of the method was.
//!
//! Emitting a row is a property of a probe's OUTPUT, so a proxy over its source
//! cannot establish it, and a tighter proxy would not either: the emission may
//! live in any module the binary calls. The rung that can enforce this is the
//! one that crosses a process boundary, which is this one.
//!
//! The binary list comes from `Cargo.toml` rather than from a list kept here, so
//! a probe added tomorrow joins the census without anyone remembering to add it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The probes whose report carries an `x-probe-*` NDJSON line.
///
/// Measured, not recalled. A document that describes the split -- `M4.10` in
/// CHECKLIST.md is the current one -- is corrected by this failing rather than
/// by somebody noticing.
const EMIT_A_ROW: [&str; 3] = [
    "probe-doorbell-cost",
    "probe-request-cost",
    "probe-topology",
];

/// The probes this test does not run, and why each one is excluded.
///
/// A list of pairs rather than a bare name, because an exclusion without its
/// reason is indistinguishable from an oversight -- and the second entry here
/// was exactly that until a review found it.
///
/// - `probe-queue-contention` is the contention benchmark: a host-dependent run
///   its own module doc puts at about sixty-five seconds, with no short mode.
///   Ten seconds for the rest is worth paying every time; seventy-five is not.
/// - `probe-cancel-io` measures a call that **can fail to return**. Its own
///   module doc says so and draws the conclusion: "Binary only, and deliberately
///   not a test ... a wedged `#[test]` would take the whole suite with it." It
///   guards each case with an internal watchdog, but `Command::output` waits on
///   the child without one, so running it here would reintroduce the hazard that
///   probe is shaped to avoid.
///
/// Both are still censused, by the one question a source can answer soundly --
/// whether the emission literal appears anywhere in their sources at all. That
/// is weaker than running them, and it is stated as weaker rather than blended
/// in with the rest.
const NOT_RUN: [(&str, &str); 2] = [
    ("probe-queue-contention", "src/bin/queue_contention"),
    ("probe-cancel-io", "src/bin/cancel_io.rs"),
];

/// Every `[[bin]]` name the manifest registers.
fn registered_binaries(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_bin = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
            continue;
        }
        if !in_bin {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name") {
            let value = rest.trim_start().trim_start_matches('=').trim();
            names.push(value.trim_matches('"').to_owned());
        }
    }
    names
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The directory cargo put this run's binaries in.
///
/// Taken from a binary cargo itself resolved, so it is right under `--release`,
/// under a custom `CARGO_TARGET_DIR`, and inside cargo-mutants' scratch copy.
fn binary_directory() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_probe-topology"))
        .parent()
        .expect("a built binary has a parent directory")
        .to_path_buf()
}

/// Every `.rs` file under `root`, at any depth.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}

#[test]
fn the_probes_that_emit_a_machine_readable_row_are_the_ones_we_say_they_are() {
    let manifest = std::fs::read_to_string(crate_root().join("Cargo.toml"))
        .expect("the crate's manifest is readable");
    let registered = registered_binaries(&manifest);
    assert!(
        registered.len() > 1,
        "no `[[bin]]` targets were read from the manifest, so this test checked \
         nothing -- the manifest's shape has probably changed"
    );
    for (name, _) in NOT_RUN {
        assert!(
            registered.iter().any(|registered| registered == name),
            "`{name}` is excluded by name, so it must still be a registered \
             target; if it was renamed, this exclusion is silently covering a \
             probe nobody is censusing"
        );
    }

    let directory = binary_directory();
    let mut emitting: Vec<String> = Vec::new();
    for name in &registered {
        if NOT_RUN.iter().any(|(excluded, _)| excluded == name) {
            continue;
        }
        let executable = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("{} could not be run: {error}", executable.display()));
        assert!(
            output.status.success(),
            "{name} exited {:?}; a probe that cannot run cannot be censused",
            output.status.code()
        );
        // The row as it is actually written, not the bare tag: a probe that only
        // mentions `x-probe` in prose is not a probe that emits one, and that
        // exact confusion is why this test moved off a source grep.
        if String::from_utf8_lossy(&output.stdout).contains(r#""reason":"x-probe"#) {
            emitting.push(name.clone());
        }
    }
    emitting.sort();

    assert_eq!(
        emitting, EMIT_A_ROW,
        "the set of probes emitting a machine-readable row has changed; update \
         `EMIT_A_ROW` and every document that describes the split, rather than \
         leaving the two to disagree"
    );

    // The excluded probes, by the weaker question. If an emission literal ever
    // appears in their sources this stops being answerable without running
    // them, and the assertion says so rather than quietly going stale.
    for (name, source_path) in NOT_RUN {
        let path = crate_root().join(source_path);
        // **One read for both shapes, and it fails loudly.** These were two
        // reads with opposite failure behaviour: the file branch panicked on an
        // unreadable source, the directory branch mapped it to an empty string
        // through `unwrap_or_default`. An empty string contains no emission
        // literal, so a source this census could not read passed it -- and the
        // non-empty guard below passed too, because the vector still had an
        // entry. The weaker half of the census could therefore report a clean
        // answer having read nothing, which is the one outcome it must not have.
        let read = |file: &Path| -> String {
            std::fs::read_to_string(file).unwrap_or_else(|error| {
                panic!(
                    "{name}'s source at {} could not be read: {error} -- the \
                     source census cannot stand in for running it if it cannot \
                     read it",
                    file.display()
                )
            })
        };
        let sources: Vec<String> = if path.is_dir() {
            walk(&path).iter().map(|file| read(file)).collect()
        } else {
            vec![read(&path)]
        };
        assert!(
            !sources.is_empty(),
            "no sources were found for `{name}` at {}, so its half of the \
             census checked nothing",
            path.display()
        );
        assert!(
            sources
                .iter()
                .all(|source| !source.contains(r#""reason":"x-probe"#)),
            "`{name}` has gained an emission literal, so the source question can \
             no longer stand in for running it -- either run it here and accept \
             the cost, or state plainly that it is uncensused"
        );
    }
}
