// Copyright (c) 2026 Mike Grier

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{CREATE_SUSPENDED, WaitForSingleObject};

use super::resume_threads;

fn spawn(program: &str, args: &[&str], flags: u32) -> Child {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(flags)
        .spawn()
        .expect("start the process")
}

fn exits_within(child: &Child, ms: u32) -> bool {
    // SAFETY: the handle is live for as long as `child` is.
    unsafe { WaitForSingleObject(child.as_raw_handle(), ms) == WAIT_OBJECT_0 }
}

#[test]
fn a_suspended_process_runs_once_resumed() {
    let mut child = spawn("cmd", &["/c", "exit", "5"], CREATE_SUSPENDED);
    assert!(
        !exits_within(&child, 200),
        "it was suspended, so it cannot have run"
    );
    assert_eq!(resume_threads(child.id()).expect("resume"), 1);
    assert!(exits_within(&child, 5_000), "resumed, it ran to its exit");
    assert_eq!(child.wait().unwrap().code(), Some(5));
}

#[test]
fn resuming_a_running_process_is_harmless() {
    let mut child = spawn("ping", &["-n", "30", "127.0.0.1"], 0);
    assert!(resume_threads(child.id()).expect("resume") >= 1);
    assert!(!exits_within(&child, 0), "still running as before");
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn an_exited_process_has_no_threads_to_resume() {
    // Its pid cannot be reused while `child` holds the process handle.
    let mut child = spawn("cmd", &["/c", "exit", "0"], 0);
    child.wait().unwrap();
    assert_eq!(resume_threads(child.id()).expect("snapshot"), 0);
}

#[test]
fn only_the_named_process_is_resumed() {
    let mut first = spawn("cmd", &["/c", "exit", "1"], CREATE_SUSPENDED);
    let mut second = spawn("cmd", &["/c", "exit", "2"], CREATE_SUSPENDED);
    assert_eq!(resume_threads(first.id()).expect("resume"), 1);
    assert!(exits_within(&first, 5_000));
    assert!(!exits_within(&second, 200), "the other stayed suspended");
    second.kill().unwrap();
    first.wait().unwrap();
    second.wait().unwrap();
}
