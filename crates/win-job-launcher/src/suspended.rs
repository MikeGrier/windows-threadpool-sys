// Copyright (c) 2026 Mike Grier
//! Resuming a process that was created suspended.
//!
//! `std::process::Child` does not expose the main-thread handle that
//! `CreateProcessW` returns (`main_thread_handle` is unstable on the pinned
//! toolchain), so the process's threads are found through a Toolhelp snapshot
//! and resumed by id. A process created suspended has only the thread
//! `CreateProcessW` gave it; every thread found is resumed regardless, and
//! resuming one that is not suspended does nothing.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{FALSE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

#[cfg(test)]
mod tests;

/// `ResumeThread`'s failure value.
const RESUME_FAILED: u32 = u32::MAX;

/// Resumes every thread of process `pid`, returning how many were found.
///
/// The caller must hold a handle to the process, so that `pid` cannot have
/// been reused by another process between creation and this call.
///
/// # Errors
///
/// The OS error from taking the snapshot, opening a thread, or resuming it.
pub fn resume_threads(pid: u32) -> io::Result<u32> {
    // SAFETY: the flags are documented; process id 0 means "all processes",
    // which is what a thread snapshot always covers.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful snapshot is a handle this process owns, released
    // with CloseHandle, which OwnedHandle does.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };

    // SAFETY: an all-zero THREADENTRY32 is valid plain data; dwSize is then set
    // as Thread32First requires.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = u32::try_from(size_of::<THREADENTRY32>()).expect("THREADENTRY32 fits a u32");

    let mut resumed = 0;
    // SAFETY: the snapshot is live and `entry` is a correctly sized, writable
    // THREADENTRY32 for each call.
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            resume_thread(entry.th32ThreadID)?;
            resumed += 1;
        }
        // SAFETY: as above. FALSE at the end of the list is the normal stop.
        more = unsafe { Thread32Next(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    }
    Ok(resumed)
}

fn resume_thread(tid: u32) -> io::Result<()> {
    // SAFETY: OpenThread takes a thread id and returns null on failure, which
    // is checked.
    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, FALSE, tid) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful OpenThread returns a handle this process owns.
    let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the handle is live and has THREAD_SUSPEND_RESUME.
    if unsafe { ResumeThread(thread.as_raw_handle()) } == RESUME_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
