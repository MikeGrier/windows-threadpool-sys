// Copyright (c) 2026 Mike Grier
//! The launcher over real process trees.
//!
//! These cross a process boundary by design: what is under test is what
//! happens to processes the launcher did not create directly. Death is checked
//! independently of the launcher's own report, through a marker file that a
//! descendant holds open: an exclusive open of it fails while any process holds
//! it. Each such test also checks the marker IS held while the tree runs, so a
//! descendant that never started cannot pass as one that was killed.
//!
//! Trees are built from `.cmd` scripts rather than `cmd /c` command lines, so
//! no test depends on how `cmd` re-parses quotes.

#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_win-job-launcher");

/// A long-running command: `ping` waits about a second between echoes.
const LONG: [&str; 4] = ["ping", "-n", "60", "127.0.0.1"];

/// A scratch directory unique to one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "win-job-launcher-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the scratch directory");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Writes a `.cmd` script; `{dir}` in `body` becomes this directory.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path(name);
        let text = format!(
            "@echo off\r\n{}\r\n",
            body.replace("{dir}", &self.0.display().to_string())
        );
        fs::write(&path, text.replace('\n', "\r\n").replace("\r\r\n", "\r\n")).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One finished launcher run.
struct Run {
    output: Output,
    result: Option<String>,
    stdout: String,
    stderr: String,
    wall: Duration,
}

impl Run {
    fn result(&self) -> &str {
        self.result.as_deref().unwrap_or_else(|| {
            panic!(
                "no result file; launcher stderr: {}",
                self.launcher_stderr()
            )
        })
    }

    fn launcher_stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }
}

fn launcher(s: &Scratch, timeout_ms: u32, trace: bool, command: &[&str]) -> Command {
    let mut c = Command::new(LAUNCHER);
    c.current_dir(&s.0)
        .stdin(Stdio::null())
        .arg("--timeout-ms")
        .arg(timeout_ms.to_string())
        .arg("--stdout")
        .arg(s.path("out.txt"))
        .arg("--stderr")
        .arg(s.path("err.txt"))
        .arg("--result")
        .arg(s.path("result.json"));
    if trace {
        c.arg("--trace");
    }
    c.arg("--").args(command);
    c
}

fn finish(s: &Scratch, start: Instant, output: Output) -> Run {
    Run {
        wall: start.elapsed(),
        result: fs::read_to_string(s.path("result.json")).ok(),
        stdout: fs::read_to_string(s.path("out.txt")).unwrap_or_default(),
        stderr: fs::read_to_string(s.path("err.txt")).unwrap_or_default(),
        output,
    }
}

fn launch(s: &Scratch, timeout_ms: u32, trace: bool, command: &[&str]) -> Run {
    let start = Instant::now();
    let output = launcher(s, timeout_ms, trace, command)
        .output()
        .expect("run the launcher");
    finish(s, start, output)
}

/// Starts the launcher without waiting, for a test that must look at the tree
/// while it runs.
fn spawn(s: &Scratch, timeout_ms: u32, command: &[&str]) -> (Child, Instant) {
    let child = launcher(s, timeout_ms, false, command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the launcher");
    (child, Instant::now())
}

/// Whether an exclusive open of `marker` succeeds -- or it does not exist --
/// which shows no process holds it.
fn free_now(marker: &Path) -> bool {
    match OpenOptions::new().read(true).share_mode(0).open(marker) {
        Ok(_) => true,
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}

/// [`free_now`], retried briefly: a terminated process's handles close as it
/// finishes terminating.
fn becomes_free(marker: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if free_now(marker) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Waits until `marker` exists and is held, which shows its holder is running.
fn wait_until_held(marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(marker.exists() && !free_now(marker)) {
        assert!(
            Instant::now() < deadline,
            "{} was never held",
            marker.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// A script that holds `marker.txt` open for as long as its ping runs.
fn holder(s: &Scratch) -> PathBuf {
    s.script("holder.cmd", "ping -n 60 127.0.0.1 > \"{dir}\\marker.txt\"")
}

#[test]
fn a_command_that_exits_reports_its_code() {
    let s = Scratch::new("exit");
    let run = launch(&s, 30_000, false, &["cmd", "/d", "/c", "exit", "3"]);
    assert_eq!(
        run.output.status.code(),
        Some(0),
        "{}",
        run.launcher_stderr()
    );
    assert!(
        run.result()
            .starts_with(r#"{"outcome":"exited","code":3,"strays":0,"confirmed":true,"#),
        "{}",
        run.result()
    );
}

#[test]
fn an_ntstatus_exit_code_is_reported_signed() {
    let s = Scratch::new("ntstatus");
    let run = launch(
        &s,
        30_000,
        false,
        &["cmd", "/d", "/c", "exit", "-1073741502"],
    );
    assert!(
        run.result().contains(r#""code":-1073741502,"#),
        "{}",
        run.result()
    );
}

#[test]
fn the_commands_stdout_and_stderr_go_to_their_files() {
    let s = Scratch::new("streams");
    let script = s.script("streams.cmd", "echo to-out\necho to-err 1>&2");
    let run = launch(&s, 30_000, false, &[script.to_str().unwrap()]);
    assert_eq!(run.stdout.trim(), "to-out");
    assert_eq!(run.stderr.trim(), "to-err");
    assert!(
        run.launcher_stderr().is_empty(),
        "untraced, the launcher says nothing: {}",
        run.launcher_stderr()
    );
}

#[test]
fn the_command_runs_in_the_launchers_working_directory() {
    let s = Scratch::new("cwd");
    let run = launch(&s, 30_000, false, &["cmd", "/d", "/c", "cd"]);
    assert_eq!(
        fs::canonicalize(run.stdout.trim()).unwrap(),
        fs::canonicalize(&s.0).unwrap()
    );
}

#[test]
fn arguments_reach_the_command_whole() {
    let s = Scratch::new("args");
    let run = launch(
        &s,
        30_000,
        false,
        &["cmd", "/d", "/c", "echo", "a b", "--trace"],
    );
    assert_eq!(run.stdout.trim(), r#""a b" --trace"#);
}

#[test]
fn a_batch_file_runs_with_its_exit_code() {
    let s = Scratch::new("batch");
    let script = s.script("stub.cmd", "echo from-batch\nexit /b 7");
    let run = launch(&s, 30_000, false, &[script.to_str().unwrap()]);
    assert!(run.result().contains(r#""code":7,"#), "{}", run.result());
    assert_eq!(run.stdout.trim(), "from-batch");
}

#[test]
fn the_bound_kills_the_command_and_reports_a_timeout() {
    let s = Scratch::new("bound");
    let run = launch(&s, 300, false, &LONG);
    assert!(
        run.result()
            .starts_with(r#"{"outcome":"timed-out","confirmed":true,"#),
        "{}",
        run.result()
    );
    assert!(
        run.wall < Duration::from_secs(30),
        "a 300ms bound took {:?}",
        run.wall
    );
}

#[test]
fn the_bound_kills_a_grandchild() {
    // The script's cmd holds the marker; its ping is the grandchild.
    let s = Scratch::new("grandchild");
    let script = holder(&s);
    let (child, start) = spawn(&s, 3_000, &[script.to_str().unwrap()]);
    let marker = s.path("marker.txt");
    wait_until_held(&marker);
    let run = finish(&s, start, child.wait_with_output().unwrap());
    assert!(
        run.result().contains(r#""outcome":"timed-out""#),
        "{}",
        run.result()
    );
    assert!(
        becomes_free(&marker),
        "the grandchild still holds the marker"
    );
}

#[test]
fn the_bound_kills_a_descendant_whose_parent_has_exited() {
    // The case a parent-PID walk cannot reach. `middle` starts the holder and
    // exits at once, so the holder's parent no longer exists; the top script
    // keeps running so that the bound, not an exit, ends the run.
    let s = Scratch::new("orphan");
    holder(&s);
    s.script(
        "middle.cmd",
        "start \"\" /b cmd /d /c \"{dir}\\holder.cmd\"\nexit /b 0",
    );
    let top = s.script(
        "top.cmd",
        "start \"\" /b cmd /d /c \"{dir}\\middle.cmd\"\nping -n 60 127.0.0.1 >nul",
    );
    let (child, start) = spawn(&s, 3_000, &[top.to_str().unwrap()]);
    let marker = s.path("marker.txt");
    wait_until_held(&marker);
    let run = finish(&s, start, child.wait_with_output().unwrap());
    assert!(
        run.result().contains(r#""outcome":"timed-out""#),
        "{}",
        run.result()
    );
    assert!(becomes_free(&marker), "the orphan still holds the marker");
}

#[test]
fn processes_left_behind_when_the_command_exits_are_killed_and_counted() {
    // The command starts a detached holder, waits until it holds the marker,
    // and exits -- so the holder is still running when the command exits.
    let s = Scratch::new("strays");
    holder(&s);
    let top = s.script(
        "top.cmd",
        "start \"\" /b cmd /d /c \"{dir}\\holder.cmd\"\n\
         :wait\n\
         if exist \"{dir}\\marker.txt\" exit /b 0\n\
         ping -n 1 127.0.0.1 >nul\n\
         goto wait",
    );
    let run = launch(&s, 30_000, false, &[top.to_str().unwrap()]);
    let result = run.result();
    assert!(
        result.contains(r#""outcome":"exited","code":0,"#),
        "{result}"
    );
    assert!(
        !result.contains(r#""strays":0,"#),
        "no stray was counted: {result}"
    );
    assert!(
        result.contains(r#""confirmed":true,"#),
        "the cleanup was not confirmed: {result}"
    );
    assert!(
        becomes_free(&s.path("marker.txt")),
        "the stray still holds the marker"
    );
}

#[test]
fn a_command_that_cannot_be_found_is_not_started() {
    let s = Scratch::new("missing");
    let run = launch(&s, 30_000, false, &["win-job-launcher-no-such-program-xyz"]);
    assert_eq!(run.output.status.code(), Some(0));
    assert!(
        run.result()
            .starts_with(r#"{"outcome":"not-started","error":"could not start the command: "#),
        "{}",
        run.result()
    );
    // The launcher finds the program itself, so a miss carries the Windows
    // error code: ERROR_FILE_NOT_FOUND.
    assert!(run.result().contains(r#""osError":2,"#), "{}", run.result());
}

#[test]
fn an_unwritable_stdout_is_not_started() {
    let s = Scratch::new("stdout");
    let output = Command::new(LAUNCHER)
        .args(["--timeout-ms", "1000", "--stdout"])
        .arg(s.path("no such dir").join("out.txt"))
        .arg("--stderr")
        .arg(s.path("err.txt"))
        .arg("--result")
        .arg(s.path("result.json"))
        .args(["--", "cmd", "/c", "exit", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = fs::read_to_string(s.path("result.json")).unwrap();
    assert!(
        text.starts_with(r#"{"outcome":"not-started","error":"could not create --stdout: "#),
        "{text}"
    );
}

#[test]
fn a_refused_command_line_exits_2_and_writes_no_result() {
    let s = Scratch::new("usage");
    let result = s.path("result.json");
    let output = Command::new(LAUNCHER)
        .args(["--timeout-ms", "0", "--result"])
        .arg(&result)
        .args(["--", "cmd"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!result.exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--timeout-ms") && stderr.contains("usage:"),
        "{stderr}"
    );
}

#[test]
fn an_unwritable_result_exits_3() {
    let s = Scratch::new("result");
    let output = Command::new(LAUNCHER)
        .args(["--timeout-ms", "10000", "--stdout"])
        .arg(s.path("out.txt"))
        .arg("--stderr")
        .arg(s.path("err.txt"))
        .arg("--result")
        .arg(s.path("no such dir").join("result.json"))
        .args(["--", "cmd", "/c", "exit", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn a_stale_result_from_an_earlier_run_is_replaced() {
    let s = Scratch::new("stale");
    fs::write(s.path("result.json"), "stale").unwrap();
    let run = launch(&s, 30_000, false, &["cmd", "/d", "/c", "exit", "0"]);
    assert!(
        run.result().starts_with(r#"{"outcome":"exited""#),
        "{}",
        run.result()
    );
}

#[test]
fn trace_narrates_each_step_of_a_kill() {
    let s = Scratch::new("trace");
    let run = launch(&s, 300, true, &LONG);
    let trace = run.launcher_stderr();
    for step in [
        "created a kill-on-close job",
        " in the job",
        "the bound of 300ms was reached; job: ",
        "calling TerminateJobObject",
        "TerminateJobObject returned",
        "the command's handle was signalled",
        "the job is empty",
        "wrote ",
    ] {
        assert!(trace.contains(step), "missing {step:?} in:\n{trace}");
    }
    assert!(
        !trace.contains("suspended") && !trace.contains("resumed"),
        "the command is created in the job, not suspended and resumed: {trace}"
    );
    assert!(
        trace.lines().all(|l| l.starts_with("win-job-launcher +")),
        "{trace}"
    );
}

#[test]
fn trace_narrates_an_exit() {
    let s = Scratch::new("trace-exit");
    let run = launch(&s, 30_000, true, &["cmd", "/d", "/c", "exit", "4"]);
    let trace = run.launcher_stderr();
    assert!(trace.contains("the command exited with code 4"), "{trace}");
    assert!(!trace.contains("TerminateJobObject"), "{trace}");
}
