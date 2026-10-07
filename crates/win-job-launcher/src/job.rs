// Copyright (c) 2026 Mike Grier
//! A Windows job object that kills its processes when it is closed.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};

use crate::outcome::Accounting;

#[cfg(test)]
mod tests;

/// `JOBOBJECT_BASIC_ACCOUNTING_INFORMATION` counts CPU time in 100 ns ticks.
const TICKS_PER_MS: i64 = 10_000;

/// An anonymous job object, kill-on-close: when the last handle to it closes --
/// including because this process died -- every process still in it is
/// terminated.
#[derive(Debug)]
pub struct Job(OwnedHandle);

impl Job {
    /// Creates the job and sets it kill-on-close.
    ///
    /// # Errors
    ///
    /// The OS error from `CreateJobObjectW` or `SetInformationJobObject`.
    pub fn new_kill_on_close() -> io::Result<Self> {
        // SAFETY: no security attributes and no name are both documented as
        // valid; the result is checked before use.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful CreateJobObjectW returns a new handle this
        // process owns, released with CloseHandle, which OwnedHandle does.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });

        // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a valid
        // value -- it is plain data -- and means "no limits".
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the handle is live; `limits` is the structure this class
        // names, and its exact size is passed.
        let set = unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast::<c_void>(),
                size_of_u32::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Puts `process` in the job. Processes it creates afterwards join too.
    ///
    /// # Errors
    ///
    /// The OS error from `AssignProcessToJobObject`.
    pub fn assign(&self, process: BorrowedHandle<'_>) -> io::Result<()> {
        // SAFETY: both handles are live for the call.
        if unsafe { AssignProcessToJobObject(self.raw(), process.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Terminates every process in the job, giving each `exit_code`. Returns
    /// once termination has been initiated; [`Job::accounting`] shows when it
    /// has finished.
    ///
    /// # Errors
    ///
    /// The OS error from `TerminateJobObject`.
    pub fn terminate(&self, exit_code: u32) -> io::Result<()> {
        // SAFETY: the handle is live for the call.
        if unsafe { TerminateJobObject(self.raw(), exit_code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The job's process counts and CPU time so far.
    ///
    /// # Errors
    ///
    /// The OS error from `QueryInformationJobObject`.
    pub fn accounting(&self) -> io::Result<Accounting> {
        // SAFETY: as for the limits above, an all-zero structure is valid.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is live; `info` is the structure this class names,
        // its exact size is passed, and the returned-length pointer may be null.
        let ok = unsafe {
            QueryInformationJobObject(
                self.raw(),
                JobObjectBasicAccountingInformation,
                ptr::from_mut(&mut info).cast::<c_void>(),
                size_of_u32::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>(),
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Accounting {
            total_processes: info.TotalProcesses,
            active_processes: info.ActiveProcesses,
            user_cpu_ms: ticks_to_ms(info.TotalUserTime),
            kernel_cpu_ms: ticks_to_ms(info.TotalKernelTime),
        })
    }

    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0.as_raw_handle()
    }
}

fn ticks_to_ms(ticks: i64) -> u64 {
    // The kernel never reports negative CPU time; clamp rather than wrap if it
    // ever did.
    u64::try_from(ticks / TICKS_PER_MS).unwrap_or(0)
}

fn size_of_u32<T>() -> u32 {
    u32::try_from(size_of::<T>()).expect("a Win32 structure's size fits a u32")
}
