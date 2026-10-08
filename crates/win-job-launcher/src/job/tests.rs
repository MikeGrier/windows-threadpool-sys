// Copyright (c) 2026 Mike Grier

use std::os::windows::io::{AsHandle, AsRawHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{CREATE_SUSPENDED, WaitForSingleObject};

use super::Job;
use crate::outcome::Accounting;

/// A `cmd /c exit <code>` created suspended, so it is alive but runs nothing
/// until resumed or killed.
fn suspended(code: i32) -> Child {
    Command::new("cmd")
        .args(["/c", "exit", &code.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_SUSPENDED)
        .spawn()
        .expect("start cmd suspended")
}

fn exits_within(child: &Child, ms: u32) -> bool {
    // SAFETY: the handle is live for as long as `child` is.
    unsafe { WaitForSingleObject(child.as_raw_handle(), ms) == WAIT_OBJECT_0 }
}

#[test]
fn a_new_job_is_empty() {
    let job = Job::new_kill_on_close().expect("create a job");
    assert_eq!(job.accounting().unwrap(), Accounting::default());
}

#[test]
fn terminating_an_empty_job_succeeds() {
    let job = Job::new_kill_on_close().expect("create a job");
    job.terminate(1).expect("terminate an empty job");
    assert_eq!(job.accounting().unwrap().active_processes, 0);
}

#[test]
fn an_assigned_process_is_counted() {
    let job = Job::new_kill_on_close().expect("create a job");
    let mut child = suspended(0);
    job.assign(child.as_handle()).expect("assign");
    let a = job.accounting().unwrap();
    assert_eq!((a.total_processes, a.active_processes), (1, 1));
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn terminating_the_job_kills_its_process_with_the_given_code() {
    let job = Job::new_kill_on_close().expect("create a job");
    let mut child = suspended(0);
    job.assign(child.as_handle()).expect("assign");
    job.terminate(77).expect("terminate");
    assert_eq!(child.wait().unwrap().code(), Some(77));
}

#[test]
fn closing_the_job_kills_its_process() {
    let job = Job::new_kill_on_close().expect("create a job");
    let mut child = suspended(0);
    job.assign(child.as_handle()).expect("assign");
    assert!(!exits_within(&child, 0), "suspended, so still alive");
    drop(job);
    assert!(
        exits_within(&child, 5_000),
        "kill-on-close took the process down"
    );
    child.wait().unwrap();
}

#[test]
fn a_new_job_lists_no_processes() {
    let job = Job::new_kill_on_close().expect("create a job");
    assert_eq!(job.process_ids().unwrap(), Vec::<u32>::new());
}

#[test]
fn an_assigned_process_is_listed_by_its_id() {
    let job = Job::new_kill_on_close().expect("create a job");
    let mut child = suspended(0);
    job.assign(child.as_handle()).expect("assign");
    assert_eq!(job.process_ids().unwrap(), [child.id()]);
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn every_assigned_process_is_listed_and_only_those() {
    let job = Job::new_kill_on_close().expect("create a job");
    let mut children: Vec<Child> = (0..5).map(|_| suspended(0)).collect();
    for child in &children {
        job.assign(child.as_handle()).expect("assign");
    }
    let outside = suspended(0);
    let mut listed = job.process_ids().unwrap();
    listed.sort_unstable();
    let mut expected: Vec<u32> = children.iter().map(Child::id).collect();
    expected.sort_unstable();
    assert_eq!(listed, expected);
    assert!(!listed.contains(&outside.id()));
    for child in &mut children {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    let mut outside = outside;
    outside.kill().unwrap();
    outside.wait().unwrap();
}

#[test]
fn a_list_longer_than_the_room_made_for_it_is_read_whole() {
    // Room for one id, with five processes: the first call is too small, and
    // the retry has to be sized from what the OS reported.
    let job = Job::new_kill_on_close().expect("create a job");
    let mut children: Vec<Child> = (0..5).map(|_| suspended(0)).collect();
    for child in &children {
        job.assign(child.as_handle()).expect("assign");
    }
    let mut listed = job.process_ids_with_capacity(1).unwrap();
    listed.sort_unstable();
    let mut expected: Vec<u32> = children.iter().map(Child::id).collect();
    expected.sort_unstable();
    assert_eq!(listed, expected);
    for child in &mut children {
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

#[test]
fn a_terminated_process_leaves_the_list() {
    let job = Job::new_kill_on_close().expect("create a job");
    let mut child = suspended(0);
    job.assign(child.as_handle()).expect("assign");
    job.terminate(1).unwrap();
    child.wait().unwrap();
    // The list, like the count, may name the process for a moment after it is
    // gone; it must be empty soon.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !job.process_ids().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "still listed after 5s"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn contains_says_whether_a_process_is_in_the_job() {
    let job = Job::new_kill_on_close().expect("create a job");
    let mut child = suspended(0);
    assert!(
        !job.contains(child.as_handle()).unwrap(),
        "not yet assigned"
    );
    job.assign(child.as_handle()).expect("assign");
    assert!(job.contains(child.as_handle()).unwrap(), "assigned");
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn a_process_that_has_already_exited_cannot_be_assigned() {
    let job = Job::new_kill_on_close().expect("create a job");
    let mut child = suspended(0);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(job.assign(child.as_handle()).is_err());
    assert_eq!(job.accounting().unwrap().total_processes, 0);
}
