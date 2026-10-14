// Copyright (c) 2026 Mike Grier

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    GetHandleInformation, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use super::{
    Process, batch_plan, cmd_exe, exe_plan, is_batch, known_directory, path_directories, push_arg,
    push_batch_part, resolve, resolve_in, search_directories, spawn_in_job, system_directory,
    windows_directory,
};
use crate::job::Job;

fn quoted(arg: &str) -> String {
    let mut out = Vec::new();
    push_arg(&mut out, OsStr::new(arg)).expect("quote the argument");
    String::from_utf16(&out).expect("valid UTF-16")
}

/// A wide string without its terminating NUL, as text.
fn text(units: &[u16]) -> String {
    let (nul, body) = units.split_last().expect("a terminator");
    assert_eq!(*nul, 0, "NUL-terminated");
    String::from_utf16(body).expect("valid UTF-16")
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn a_plain_argument_is_not_quoted() {
    assert_eq!(quoted("abc"), "abc");
    assert_eq!(quoted("--flag=value"), "--flag=value");
}

#[test]
fn an_empty_argument_is_an_empty_pair_of_quotes() {
    assert_eq!(quoted(""), r#""""#);
}

#[test]
fn spaces_and_tabs_make_an_argument_quoted() {
    assert_eq!(quoted("a b"), r#""a b""#);
    assert_eq!(quoted("a\tb"), "\"a\tb\"");
    assert_eq!(quoted(" "), r#"" ""#);
}

#[test]
fn an_embedded_quote_is_backslash_escaped_without_outer_quotes() {
    assert_eq!(quoted(r#"a"b"#), r#"a\"b"#);
    assert_eq!(quoted(r#"""#), r#"\""#);
}

#[test]
fn backslashes_before_a_quote_are_doubled_and_the_quote_escaped() {
    assert_eq!(quoted(r#"a\"b"#), r#"a\\\"b"#);
    assert_eq!(quoted(r#"a\\"b"#), r#"a\\\\\"b"#);
}

#[test]
fn backslashes_not_before_a_quote_are_left_alone() {
    assert_eq!(quoted(r"C:\a\b"), r"C:\a\b");
    assert_eq!(quoted(r"a\"), r"a\");
}

#[test]
fn trailing_backslashes_are_doubled_inside_the_closing_quote() {
    assert_eq!(quoted(r"a b\"), r#""a b\\""#);
    assert_eq!(quoted(r"a b\\"), r#""a b\\\\""#);
}

#[test]
fn text_outside_ascii_passes_through() {
    assert_eq!(quoted("caf\u{e9} \u{4e2d}"), "\"caf\u{e9} \u{4e2d}\"");
}

#[test]
fn an_unpaired_surrogate_is_preserved_rather_than_replaced() {
    let arg = OsString::from_wide(&[0x61, 0xd800, 0x62]);
    let mut out = Vec::new();
    push_arg(&mut out, &arg).unwrap();
    assert_eq!(out, [0x61, 0xd800, 0x62]);
}

#[test]
fn a_nul_in_an_argument_is_refused() {
    let arg = OsString::from_wide(&[0x61, 0, 0x62]);
    let e = push_arg(&mut Vec::new(), &arg).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn an_executable_is_run_directly_under_its_own_path() {
    let plan = exe_plan(
        Path::new(r"C:\Program Files\x\tool.exe"),
        &args(&["a", "b c", ""]),
    )
    .unwrap();
    assert_eq!(
        text(&plan.command_line),
        r#""C:\Program Files\x\tool.exe" a "b c" """#
    );
    assert_eq!(text(&plan.application), r"C:\Program Files\x\tool.exe");
}

#[test]
fn a_program_path_holding_a_quote_is_refused() {
    let e = exe_plan(Path::new(r#"C:\a"b\tool.exe"#), &[]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn only_cmd_and_bat_files_are_batch_files() {
    for yes in ["a.cmd", "a.CMD", "a.Bat", r"C:\x y\a.bat"] {
        assert!(is_batch(Path::new(yes)), "{yes}");
    }
    for no in ["a.exe", "a", "a.cmd.exe", "a.com", "cmd", "a.batx"] {
        assert!(!is_batch(Path::new(no)), "{no}");
    }
}

#[test]
fn a_batch_file_runs_through_cmd_with_every_argument_quoted_whole() {
    let plan = batch_plan(
        Path::new(r"C:\Windows\System32\cmd.exe"),
        Path::new(r"C:\s p\run.cmd"),
        &args(&["a", "b c"]),
    )
    .unwrap();
    assert_eq!(
        text(&plan.command_line),
        r#""C:\Windows\System32\cmd.exe" /e:ON /v:OFF /d /c ""C:\s p\run.cmd" "a" "b c"""#
    );
    assert_eq!(text(&plan.application), r"C:\Windows\System32\cmd.exe");
}

#[test]
fn trailing_backslashes_in_a_batch_files_arguments_are_doubled_before_the_closing_quote() {
    let part = |arg: &str| {
        let mut out = Vec::new();
        push_batch_part(&mut out, OsStr::new(arg)).unwrap();
        String::from_utf16(&out).unwrap()
    };
    assert_eq!(part(r"dir\"), r#""dir\\""#);
    assert_eq!(part(r"dir\\"), r#""dir\\\\""#);
    assert_eq!(part(r"C:\a b\"), r#""C:\a b\\""#);
    // Backslashes anywhere else are left as they are.
    assert_eq!(part(r"a\b"), r#""a\b""#);
    assert_eq!(part(r"\a"), r#""\a""#);
    assert_eq!(part(""), r#""""#);
}

#[test]
fn what_cmd_would_reread_as_syntax_is_refused_in_a_batch_files_arguments() {
    let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
    for bad in ["a\"b", "100%", "%PATH%", "a\nb", "a\rb"] {
        let e = batch_plan(cmd, Path::new(r"C:\run.cmd"), &args(&[bad])).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
    }
}

#[test]
fn what_cmd_would_reread_as_syntax_is_refused_in_a_batch_files_path() {
    let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
    for bad in [r"C:\100%\run.cmd", "C:\\a\"b\\run.cmd"] {
        let e = batch_plan(cmd, Path::new(bad), &[]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad}");
    }
}

#[test]
fn shell_metacharacters_are_accepted_in_a_batch_files_arguments_because_they_are_quoted() {
    let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
    for meta in ["a&b", "a|b", "a<b", "a>b", "a^b", "(a)", "a!b", "a;b"] {
        let plan = batch_plan(cmd, Path::new(r"C:\run.cmd"), &args(&[meta])).unwrap();
        let line = text(&plan.command_line);
        assert!(line.contains(&format!(" \"{meta}\"")), "{line}");
    }
}

#[test]
fn a_program_name_is_found_in_the_documented_order_with_the_system_directory_before_path() {
    let found = resolve(OsStr::new("cmd")).expect("find cmd");
    assert_eq!(found, system_directory().unwrap().join("cmd.exe"));
    assert_eq!(resolve(OsStr::new("cmd.exe")).unwrap(), found);
    assert!(found.is_absolute());
}

#[test]
fn the_search_order_is_the_launchers_directory_then_system_then_windows_then_path() {
    let directories = search_directories();
    let exe_directory = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert_eq!(directories[0], exe_directory);
    assert_eq!(directories[1], system_directory().unwrap());
    assert_eq!(directories[2], windows_directory().unwrap());
    let path = std::env::var_os("PATH").unwrap_or_default();
    let on_path: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|directory| !directory.as_os_str().is_empty())
        .collect();
    assert_eq!(&directories[3..], &on_path[..]);
}

#[test]
fn empty_path_entries_are_dropped_so_the_current_directory_cannot_be_reached_through_them() {
    let paths = |text: &str| path_directories(OsStr::new(text));
    let a = PathBuf::from(r"C:\a");
    let b = PathBuf::from(r"C:\b");
    assert_eq!(paths(r"C:\a;C:\b"), [a.clone(), b.clone()]);
    assert_eq!(
        paths(r"C:\a;;C:\b"),
        [a.clone(), b.clone()],
        "an empty entry between"
    );
    assert_eq!(
        paths(r";C:\a;C:\b"),
        [a.clone(), b.clone()],
        "a leading one"
    );
    assert_eq!(
        paths(r"C:\a;C:\b;"),
        [a.clone(), b.clone()],
        "a trailing one"
    );
    assert_eq!(paths(r";;C:\a;;;C:\b;;"), [a, b], "several of each");
    assert!(paths("").is_empty());
    assert!(paths(";").is_empty());
    assert!(paths(";;;").is_empty());
}

#[test]
fn the_current_directory_is_not_searched_for_a_program() {
    // A program that exists only in the directory the search runs from is not
    // found: the search is the listed directories and nothing else.
    let s = Scratch::new("cwd");
    fs::write(s.path("only-here.exe"), "").unwrap();
    let e = resolve_in(OsStr::new("only-here"), &[s.path("elsewhere")]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
}

#[test]
fn the_first_directory_holding_the_program_wins() {
    let s = Scratch::new("order");
    for (dir, content) in [("a", "first"), ("b", "second")] {
        fs::create_dir(s.path(dir)).unwrap();
        fs::write(s.path(dir).join("tool.exe"), content).unwrap();
    }
    let found = resolve_in(OsStr::new("tool"), &[s.path("a"), s.path("b")]).unwrap();
    assert_eq!(fs::read_to_string(found).unwrap(), "first");
    let found = resolve_in(OsStr::new("tool"), &[s.path("b"), s.path("a")]).unwrap();
    assert_eq!(fs::read_to_string(found).unwrap(), "second");
}

#[test]
fn exe_is_appended_to_a_bare_name_and_nothing_else_is() {
    let s = Scratch::new("extension");
    fs::write(s.path("only.exe"), "").unwrap();
    fs::write(s.path("script.cmd"), "").unwrap();
    let dirs = [s.0.clone()];
    assert_eq!(
        resolve_in(OsStr::new("only"), &dirs).unwrap(),
        s.path("only.exe")
    );
    assert_eq!(
        resolve_in(OsStr::new("only.exe"), &dirs).unwrap(),
        s.path("only.exe")
    );
    assert_eq!(
        resolve_in(OsStr::new("script.cmd"), &dirs).unwrap(),
        s.path("script.cmd")
    );
    // Not found as `script`, which would be looked for as `script.exe`.
    let e = resolve_in(OsStr::new("script"), &dirs).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
}

#[test]
fn a_name_with_a_directory_part_is_taken_as_given_and_not_searched_for() {
    let s = Scratch::new("explicit");
    fs::create_dir(s.path("elsewhere")).unwrap();
    fs::write(s.path("elsewhere").join("tool.exe"), "").unwrap();
    let explicit = s.path("elsewhere").join("tool.exe");
    // Found whatever directories are given, including none.
    assert_eq!(resolve_in(explicit.as_os_str(), &[]).unwrap(), explicit);
    assert_eq!(
        resolve_in(s.path("elsewhere").join("tool").as_os_str(), &[]).unwrap(),
        explicit
    );
    // And a missing one is not rescued by a directory that holds the bare name.
    fs::write(s.path("tool.exe"), "").unwrap();
    let missing = s.path("nowhere").join("tool.exe");
    let e = resolve_in(missing.as_os_str(), std::slice::from_ref(&s.0)).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(2));
}

#[test]
fn a_program_that_does_not_exist_is_a_file_not_found_error() {
    let e = resolve(OsStr::new("win-job-launcher-no-such-program-xyz")).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
    assert_eq!(e.raw_os_error(), Some(2));
}

#[test]
fn the_command_interpreter_comes_from_the_system_directory() {
    let cmd = cmd_exe().unwrap();
    assert_eq!(cmd, system_directory().unwrap().join("cmd.exe"));
    assert!(cmd.is_file());
}

#[test]
fn a_known_directory_longer_than_the_first_buffer_is_read_whole() {
    // The call reports the length needed (with its NUL) while the buffer is too
    // small, then writes the text once it is not.
    let calls = std::cell::Cell::new(0);
    let found = known_directory(|buffer, length| {
        calls.set(calls.get() + 1);
        if length < 400 {
            return 400;
        }
        for i in 0..399 {
            // SAFETY: `i` is below `length`, which the caller made writable.
            unsafe { *buffer.add(i) = u16::from(b'a') };
        }
        399
    })
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(found, PathBuf::from("a".repeat(399)));
}

#[test]
fn a_known_directory_that_cannot_be_read_is_none() {
    assert_eq!(known_directory(|_, _| 0), None);
}

#[test]
fn a_program_name_holding_a_nul_is_refused_before_any_search() {
    let name = OsString::from_wide(&[0x61, 0, 0x62]);
    assert_eq!(
        resolve(&name).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

/// A scratch directory unique to one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "win-job-launcher-process-{name}-{}-{}",
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

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Starts `program` in `job`, its output going to `out.txt` and `err.txt` in
/// `s`.
fn start(s: &Scratch, job: &Job, program: &str, list: &[&str]) -> io::Result<Process> {
    let stdout = File::create(s.path("out.txt")).unwrap();
    let stderr = File::create(s.path("err.txt")).unwrap();
    spawn_in_job(job, OsStr::new(program), &args(list), &stdout, &stderr)
}

fn exits_within(process: &Process, ms: u32) -> bool {
    // SAFETY: the handle is live for as long as `process` is.
    unsafe { WaitForSingleObject(process.as_raw_handle(), ms) == WAIT_OBJECT_0 }
}

fn borrowed(process: &Process) -> BorrowedHandle<'_> {
    // SAFETY: the handle is live for as long as `process` is.
    unsafe { BorrowedHandle::borrow_raw(process.as_raw_handle()) }
}

#[test]
fn the_process_is_a_member_of_the_job_the_moment_it_exists() {
    let s = Scratch::new("member");
    let job = Job::new_kill_on_close().unwrap();
    let process = start(&s, &job, "ping", &["-n", "30", "127.0.0.1"]).unwrap();
    assert!(job.contains(borrowed(&process)).unwrap());
    let a = job.accounting().unwrap();
    assert_eq!((a.total_processes, a.active_processes), (1, 1));
    job.terminate(1).unwrap();
    assert!(exits_within(&process, 5_000));
}

#[test]
fn a_process_created_in_another_job_is_not_a_member() {
    // The control for the test above: membership is shown by the call, not by
    // `contains` answering yes to everything.
    let s = Scratch::new("control");
    let job = Job::new_kill_on_close().unwrap();
    let other = Job::new_kill_on_close().unwrap();
    let process = start(&s, &other, "ping", &["-n", "30", "127.0.0.1"]).unwrap();
    assert!(!job.contains(borrowed(&process)).unwrap());
    assert_eq!(job.accounting().unwrap().total_processes, 0);
    other.terminate(1).unwrap();
    assert!(exits_within(&process, 5_000));
}

#[test]
fn closing_the_job_kills_a_process_created_in_it() {
    let s = Scratch::new("close");
    let job = Job::new_kill_on_close().unwrap();
    let process = start(&s, &job, "ping", &["-n", "30", "127.0.0.1"]).unwrap();
    assert!(!exits_within(&process, 0), "it is running");
    drop(job);
    assert!(exits_within(&process, 5_000), "kill-on-close took it down");
}

#[test]
fn the_exit_code_is_read_signed() {
    let s = Scratch::new("code");
    let job = Job::new_kill_on_close().unwrap();
    for code in [0, 3, -1, -1_073_741_502] {
        let process = start(&s, &job, "cmd", &["/d", "/c", "exit", &code.to_string()]).unwrap();
        assert!(exits_within(&process, 10_000));
        assert_eq!(process.exit_code().unwrap(), code);
    }
}

#[test]
fn the_process_id_is_the_created_processes_own() {
    let s = Scratch::new("id");
    let job = Job::new_kill_on_close().unwrap();
    let process = start(&s, &job, "cmd", &["/d", "/c", "exit", "0"]).unwrap();
    assert_ne!(process.id(), 0);
    assert_ne!(process.id(), std::process::id());
    assert!(exits_within(&process, 10_000));
}

#[test]
fn stdout_and_stderr_go_to_their_files_and_stdin_is_null() {
    let s = Scratch::new("streams");
    let job = Job::new_kill_on_close().unwrap();
    // `more` reads stdin to its end; a null stdin ends at once, so it returns
    // rather than waiting for a console.
    let process = start(
        &s,
        &job,
        "cmd",
        &["/d", "/c", "echo to-out & echo to-err 1>&2 & more"],
    )
    .unwrap();
    assert!(exits_within(&process, 10_000), "stdin was not null");
    assert_eq!(s.read("out.txt").trim(), "to-out");
    assert_eq!(s.read("err.txt").trim(), "to-err");
}

#[test]
fn arguments_reach_an_executable_whole() {
    let s = Scratch::new("args");
    let job = Job::new_kill_on_close().unwrap();
    let process = start(&s, &job, "cmd", &["/d", "/c", "echo", "a b", "--trace"]).unwrap();
    assert!(exits_within(&process, 10_000));
    assert_eq!(s.read("out.txt").trim(), r#""a b" --trace"#);
}

#[test]
fn a_batch_file_runs_with_arguments_that_are_syntax_to_cmd() {
    let s = Scratch::new("batch");
    let script = s.path("echo args.cmd");
    fs::write(&script, "@echo off\r\necho [%1] [%2] [%3]\r\nexit /b 7\r\n").unwrap();
    let job = Job::new_kill_on_close().unwrap();
    let process = start(&s, &job, script.to_str().unwrap(), &["a b", "c&d", "e|f"]).unwrap();
    assert!(exits_within(&process, 10_000));
    assert_eq!(
        process.exit_code().unwrap(),
        7,
        "out: {:?} err: {:?}",
        s.read("out.txt"),
        s.read("err.txt")
    );
    // `%1` keeps the quotes it was given, which is what shows each argument
    // reached the script whole and inert.
    assert_eq!(s.read("out.txt").trim(), r#"["a b"] ["c&d"] ["e|f"]"#);
}

#[test]
fn a_batch_file_that_forwards_its_arguments_keeps_a_trailing_backslash_from_eating_the_next() {
    // The C runtime reads `"dir\"` as a quote escaped by a backslash, so the
    // forwarded argument would swallow its neighbour. The consumer here is
    // PowerShell, which splits its command line the same way.
    let s = Scratch::new("forward");
    fs::write(
        s.path("echo.ps1"),
        "$args | ForEach-Object { \"[$_]\" }\r\n",
    )
    .unwrap();
    let script = s.path("forward.cmd");
    fs::write(
        &script,
        format!(
            "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"{}\" \"%~1\" \"%~2\"\r\n",
            s.path("echo.ps1").display()
        ),
    )
    .unwrap();
    let job = Job::new_kill_on_close().unwrap();
    let process = start(
        &s,
        &job,
        script.to_str().unwrap(),
        &[r"C:\dir with space\", "next"],
    )
    .unwrap();
    assert!(exits_within(&process, 60_000));
    let out = s.read("out.txt");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [r"[C:\dir with space\]", "[next]"],
        "out: {out:?} err: {:?}",
        s.read("err.txt")
    );
}

#[test]
fn a_batch_file_argument_cmd_would_reread_is_refused_before_anything_starts() {
    let s = Scratch::new("refused");
    let script = s.path("run.cmd");
    fs::write(&script, "@echo off\r\n").unwrap();
    let job = Job::new_kill_on_close().unwrap();
    let e = start(&s, &job, script.to_str().unwrap(), &["%PATH%"]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(job.accounting().unwrap().total_processes, 0);
}

#[test]
fn a_program_that_cannot_be_found_starts_nothing() {
    let s = Scratch::new("missing");
    let job = Job::new_kill_on_close().unwrap();
    let e = start(&s, &job, "win-job-launcher-no-such-program-xyz", &[]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
    assert_eq!(job.accounting().unwrap().total_processes, 0);
}

#[test]
fn the_callers_handles_are_left_exactly_as_they_were() {
    // Neither marked inheritable by the call, nor stripped of the flag they
    // already had: a spawn must not change what a later process creation, on
    // any thread, would inherit.
    let s = Scratch::new("flags");
    let job = Job::new_kill_on_close().unwrap();
    let plain_out = File::create(s.path("plain-out.txt")).unwrap();
    let plain_err = File::create(s.path("plain-err.txt")).unwrap();
    let marked_out = File::create(s.path("marked-out.txt")).unwrap();
    let marked_err = File::create(s.path("marked-err.txt")).unwrap();
    set_inherit(&marked_out, true);
    set_inherit(&marked_err, true);

    for (out, err, expected) in [
        (&plain_out, &plain_err, false),
        (&marked_out, &marked_err, true),
    ] {
        let process = spawn_in_job(
            &job,
            OsStr::new("cmd"),
            &args(&["/d", "/c", "exit", "0"]),
            out,
            err,
        )
        .unwrap();
        assert!(exits_within(&process, 10_000));
        assert_eq!(inherits(out), expected, "stdout");
        assert_eq!(inherits(err), expected, "stderr");
    }
}

#[test]
fn the_handles_given_to_a_command_that_failed_to_start_are_left_alone_too() {
    let s = Scratch::new("flags-failed");
    let job = Job::new_kill_on_close().unwrap();
    let out = File::create(s.path("out.txt")).unwrap();
    let err = File::create(s.path("err.txt")).unwrap();
    let e = spawn_in_job(
        &job,
        OsStr::new("win-job-launcher-no-such-program-xyz"),
        &[],
        &out,
        &err,
    )
    .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
    assert!(!inherits(&out) && !inherits(&err));
}

fn inherits(file: &File) -> bool {
    let mut flags = 0;
    // SAFETY: the handle is live and `flags` is writable.
    assert_ne!(
        unsafe { GetHandleInformation(file.as_raw_handle(), &raw mut flags) },
        0
    );
    flags & HANDLE_FLAG_INHERIT != 0
}

fn set_inherit(file: &File, on: bool) {
    let value = if on { HANDLE_FLAG_INHERIT } else { 0 };
    // SAFETY: the handle is live for the call.
    assert_ne!(
        unsafe { SetHandleInformation(file.as_raw_handle(), HANDLE_FLAG_INHERIT, value) },
        0
    );
}

#[test]
fn the_listed_streams_reach_the_commands_own_children() {
    // The three listed handles do reach a grandchild, so what it writes after
    // the command has exited still lands in the file. That nothing else is
    // inherited is checked in `tests/inheritance.rs`, which needs a process
    // of its own.
    let s = Scratch::new("grandchild");
    let script = s.path("spawn.cmd");
    fs::write(
        &script,
        "@echo off\r\nstart \"\" /b cmd /d /c \"ping -n 2 127.0.0.1 >nul & echo late\"\r\nexit /b 0\r\n",
    )
    .unwrap();
    let job = Job::new_kill_on_close().unwrap();
    let process = start(&s, &job, script.to_str().unwrap(), &[]).unwrap();
    assert!(exits_within(&process, 10_000));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !s.read("out.txt").contains("late") {
        assert!(Instant::now() < deadline, "no output from the grandchild");
        thread::sleep(Duration::from_millis(50));
    }
}
