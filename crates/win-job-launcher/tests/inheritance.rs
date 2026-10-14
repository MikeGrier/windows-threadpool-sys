// Copyright (c) 2026 Mike Grier
//! That the command inherits its three standard handles and nothing else.
//!
//! This is its own test target, and holds a single test, because an inheritable
//! handle is inherited by *any* process created while it is open -- including
//! one a sibling test creates through `std::process::Command` on another
//! thread. In a target with other tests that is a race that fails the test for
//! a reason unrelated to the code under test.

#![cfg(windows)]

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use win_job_launcher::job::Job;
use win_job_launcher::process::spawn_in_job;

#[test]
fn a_handle_the_launcher_holds_is_not_inherited_by_the_command() {
    let dir = std::env::temp_dir().join(format!("win-job-launcher-inherit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // The marker is inheritable, as a handle leaked in from elsewhere would be.
    // Only the handles named in the attribute list are inherited, so once the
    // launcher's own handle is dropped nothing holds the file and an exclusive
    // open succeeds -- while the command is still running.
    let marker = dir.join("marker.txt");
    let held = File::create(&marker).unwrap();
    // SAFETY: the handle is live for the call.
    let set = unsafe {
        SetHandleInformation(
            held.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    };
    assert_ne!(set, 0, "make the marker inheritable");

    let job = Job::new_kill_on_close().unwrap();
    let out = File::create(dir.join("out.txt")).unwrap();
    let err = File::create(dir.join("err.txt")).unwrap();
    let args = ["-n", "30", "127.0.0.1"].map(Into::into);
    let process = spawn_in_job(&job, OsStr::new("ping"), &args, &out, &err).unwrap();

    drop(held);
    // SAFETY: the handle is live for as long as `process` is.
    let running = unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != 0;
    assert!(running, "the command is still running");
    let exclusive = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&marker)
        .is_ok();
    job.terminate(1).unwrap();
    assert!(exclusive, "the command inherited the marker handle");

    let _ = fs::remove_dir_all(&dir);
}
