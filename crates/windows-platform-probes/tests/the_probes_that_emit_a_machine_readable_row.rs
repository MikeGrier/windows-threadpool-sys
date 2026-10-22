// Copyright (c) Mike Grier.

//! Every registered probe emits a machine-readable row, established by running
//! them.
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
//! a probe added tomorrow joins the census without anyone remembering to add it
//! -- and, since `M8.14`, fails it unless it emits a row, because the rule
//! (`d-every-probe-emits-a-row` in DESIGN-NOTES.md) is that every probe does.
//! Until then this test kept a list of the probes that emitted one, which a
//! probe without a row could join as easily as one with.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// What a row looks like as it is written: the start of its `reason` member.
/// Not the bare tag -- a probe that only mentions `x-probe` in prose is not a
/// probe that emits one, and that exact confusion is why this test moved off a
/// source grep.
const ROW: &str = r#""reason":"x-probe"#;

/// How long one probe may run before the census gives up on it.
///
/// **Every probe runs under it**, because `Command::output` waits on a child
/// without limit, and one probe's subject is a call that can fail to return. A
/// probe that runs out of time fails the census with its name rather than
/// hanging the suite. The longest run is the cancel-io probe's worst case,
/// several five-second watchdogs; this is several times that.
const DEADLINE: Duration = Duration::from_secs(120);

/// Probes the census does not wait to exit once their row has been read.
///
/// - `probe-cancel-io` measures a call that **can fail to return**, and its own
///   module doc draws the conclusion: a canceller wedged that way is left
///   running, since nothing can stop it safely. Whether such a thread also holds
///   up the process's exit is not something this census should stake a hang on.
///   Its report streams line by line, so its row is written before `main`
///   returns; reading the row is the census's whole question, and having read
///   it the census ends the process rather than waiting for it.
///
/// This is why the probe can be censused at all. Until `M8.14` it was excluded
/// and checked only for an emission literal in its sources -- a question a row
/// built with `Row::new` answers falsely.
const MAY_NOT_EXIT: [&str; 1] = ["probe-cancel-io"];

/// Probes run with a command line that keeps them short, and that command line.
///
/// The settings change how much is measured, never which lines the report
/// carries, so a short run answers the census's question as well as a full
/// one. One producer, one repetition and a thousand pushes take a fraction of a
/// second against the default's minute.
const SHORT_RUNS: [(&str, &[&str]); 1] = [(
    "probe-queue-contention",
    &[
        "--producers",
        "1",
        "--repetitions",
        "1",
        "--pushes-per-producer",
        "1000",
    ],
)];

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

/// Ends a child the census has given up on, without waiting for it.
///
/// Waiting is exactly what cannot be relied on for the one probe whose subject
/// can fail to return, so the child is terminated and left.
fn abandon(child: &mut Child) {
    let _ = child.kill();
}

/// Run one probe under `deadline` and say whether its report carried a row.
///
/// `may_not_exit` ends the process as soon as a row is read, for a probe in
/// [`MAY_NOT_EXIT`]; every other probe must also exit successfully in time.
///
/// # Panics
///
/// Panics, naming the probe, when it cannot be started, does not finish its
/// report in time, or exits unsuccessfully -- a probe that cannot run cannot be
/// censused.
fn emits_a_row(name: &str, mut command: Command, may_not_exit: bool, deadline: Duration) -> bool {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("{name} could not be run: {error}"));
    let stdout = child.stdout.take().expect("stdout was piped");

    // Lines arrive on another thread so the wait below has a deadline; a read
    // on the pipe itself has none.
    let (send, lines) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).split(b'\n') {
            let Ok(line) = line else { break };
            if send
                .send(String::from_utf8_lossy(&line).into_owned())
                .is_err()
            {
                break;
            }
        }
    });

    let limit = deadline;
    let deadline = Instant::now() + limit;
    let mut row = false;
    loop {
        match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => {
                if line.contains(ROW) {
                    row = true;
                    if may_not_exit {
                        abandon(&mut child);
                        return true;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                abandon(&mut child);
                panic!("{name} did not finish its report within {limit:?}");
            }
        }
    }

    // The report is closed, so the probe is exiting; it still has to say how.
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert!(
                    status.success(),
                    "{name} exited {:?}; a probe that cannot run cannot be censused",
                    status.code()
                );
                return row;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            // Not reached by a test below: it needs a child that closes its
            // own standard output and then keeps running, which no `cmd.exe`
            // one-liner does. A probe that freed its console and kept working
            // would reach it.
            Ok(None) | Err(_) => {
                abandon(&mut child);
                panic!("{name} closed its report but did not exit within {limit:?}");
            }
        }
    }
}

#[test]
fn every_registered_probe_emits_a_machine_readable_row() {
    let manifest = std::fs::read_to_string(crate_root().join("Cargo.toml"))
        .expect("the crate's manifest is readable");
    let registered = registered_binaries(&manifest);
    assert!(
        registered.len() > 1,
        "no `[[bin]]` targets were read from the manifest, so this test checked \
         nothing -- the manifest's shape has probably changed"
    );
    for name in MAY_NOT_EXIT {
        assert!(
            registered.iter().any(|registered| registered == name),
            "`{name}` is excused from exiting by name, so it must still be a \
             registered target; if it was renamed, the renamed probe would be \
             waited on, which is the hang the excuse exists to avoid"
        );
    }
    for (name, _) in SHORT_RUNS {
        assert!(
            registered.iter().any(|registered| registered == name),
            "`{name}` is given a short command line by name, so it must still be \
             a registered target; if it was renamed, the renamed probe is run at \
             its full length"
        );
    }

    let directory = binary_directory();
    let mut without_a_row: Vec<&str> = Vec::new();
    for name in &registered {
        let executable = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let arguments = SHORT_RUNS
            .iter()
            .find(|(short, _)| short == name)
            .map_or(&[][..], |(_, arguments)| *arguments);
        let mut command = Command::new(&executable);
        command.args(arguments);
        let may_not_exit = MAY_NOT_EXIT.contains(&name.as_str());
        if !emits_a_row(name, command, may_not_exit, DEADLINE) {
            without_a_row.push(name);
        }
    }

    assert!(
        without_a_row.is_empty(),
        "every probe's normal run emits a row of raw data (`d-every-probe-emits-a-row` \
         in DESIGN-NOTES.md), and these did not: {without_a_row:?}"
    );
}

/// A `cmd.exe` running `script`, with a stdin it can wait on forever.
///
/// The census's deadline paths need a child that does not finish, and `set /p`
/// blocks reading a pipe nothing writes to. It is a `cmd` builtin, so ending
/// `cmd` ends it -- no grandchild outlives the test.
fn cmd(script: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new("cmd.exe");
    command
        .raw_arg(format!("/d /c {script}"))
        .stdin(Stdio::piped());
    command
}

#[test]
fn a_report_that_never_finishes_fails_the_census_by_its_deadline() {
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(|| {
        emits_a_row(
            "hangs",
            cmd("set /p never="),
            false,
            Duration::from_millis(500),
        )
    });
    let message = *outcome
        .expect_err("a probe that never finishes must fail the census")
        .downcast::<String>()
        .expect("a formatted message");
    assert!(message.contains("did not finish its report"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the deadline bounded the wait"
    );
}

#[test]
fn a_probe_excused_from_exiting_is_ended_once_its_row_is_read() {
    // Both directions: the row is found while the process is still running, and
    // the same child, not excused, is not ended on the row -- it runs into the
    // deadline instead.
    let script = r#"echo {"reason":"x-probe-test"}& set /p never="#;
    let started = Instant::now();
    assert!(emits_a_row(
        "excused",
        cmd(script),
        true,
        Duration::from_secs(60)
    ));
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the census stopped waiting once it read the row"
    );

    let outcome = std::panic::catch_unwind(|| {
        emits_a_row(
            "not excused",
            cmd(script),
            false,
            Duration::from_millis(500),
        )
    });
    assert!(
        outcome.is_err(),
        "a probe that is not excused must still finish in time"
    );
}

#[test]
fn a_probe_that_exits_unsuccessfully_fails_the_census() {
    let outcome = std::panic::catch_unwind(|| {
        emits_a_row(
            "fails",
            cmd(r#"echo {"reason":"x-probe-test"}& exit 3"#),
            false,
            Duration::from_secs(60),
        )
    });
    let message = *outcome
        .expect_err("a probe that exits unsuccessfully must fail the census")
        .downcast::<String>()
        .expect("a formatted message");
    assert!(message.contains("exited Some(3)"), "{message}");
}

#[test]
fn a_report_is_read_as_written_whether_or_not_it_carries_a_row() {
    assert!(emits_a_row(
        "row",
        cmd(r#"echo prose& echo {"reason":"x-probe-test","n":1}"#),
        false,
        Duration::from_secs(60),
    ));
    // A mention of the tag in prose is not a row.
    assert!(!emits_a_row(
        "prose",
        cmd("echo this probe writes x-probe-test in prose only"),
        false,
        Duration::from_secs(60),
    ));
}
