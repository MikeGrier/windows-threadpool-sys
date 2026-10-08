// Copyright (c) 2026 Mike Grier
//! A Windows job object that kills its processes when it is closed.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::ptr;

use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
};

use crate::outcome::Accounting;

#[cfg(test)]
mod tests;

/// `JOBOBJECT_BASIC_ACCOUNTING_INFORMATION` counts CPU time in 100 ns ticks.
const TICKS_PER_MS: i64 = 10_000;

/// How many process ids [`Job::process_ids`] makes room for at first; it asks
/// again with room for all of them when there are more.
const INITIAL_PROCESS_IDS: usize = 64;

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

    /// Whether `process` is in the job.
    ///
    /// # Errors
    ///
    /// The OS error from `IsProcessInJob`.
    pub fn contains(&self, process: BorrowedHandle<'_>) -> io::Result<bool> {
        let mut result = 0;
        // SAFETY: both handles are live for the call, and `result` is writable.
        if unsafe { IsProcessInJob(process.as_raw_handle(), self.raw(), &raw mut result) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(result != 0)
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

    /// The ids of the processes in the job now.
    ///
    /// Unlike [`Job::accounting`]'s active count, this names them, so a caller
    /// can tell a process it knows about from one it does not. The count lags:
    /// a process that has exited -- its handle signalled -- can still be counted
    /// active for a moment afterwards.
    ///
    /// # Errors
    ///
    /// The OS error from `QueryInformationJobObject`.
    pub fn process_ids(&self) -> io::Result<Vec<u32>> {
        self.process_ids_with_capacity(INITIAL_PROCESS_IDS)
    }

    fn process_ids_with_capacity(&self, mut capacity: usize) -> io::Result<Vec<u32>> {
        loop {
            // Whole `usize` words, so the buffer is aligned for the structure
            // and its ids, which are pointer-sized.
            let bytes = size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>()
                + capacity.saturating_sub(1) * size_of::<usize>();
            let mut buffer = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
            // SAFETY: the handle is live; the buffer is writable for `bytes`
            // bytes, which is the length passed, and the returned-length
            // pointer may be null.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.raw(),
                    JobObjectBasicProcessIdList,
                    buffer.as_mut_ptr().cast::<c_void>(),
                    u32::try_from(bytes).expect("the list's size fits a u32"),
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                let e = io::Error::last_os_error();
                // The list did not fit. The header, which is written even
                // then, says how many processes there are.
                if e.raw_os_error() == Some(ERROR_MORE_DATA.cast_signed()) {
                    let list = buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
                    // SAFETY: the buffer is at least one header long and the
                    // OS wrote the header.
                    let assigned = unsafe { (*list).NumberOfAssignedProcesses } as usize;
                    capacity = assigned.max(capacity.saturating_mul(2));
                    continue;
                }
                return Err(e);
            }
            let list = buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
            // SAFETY: the call succeeded, so the header and
            // `NumberOfProcessIdsInList` ids after it are written; the ids
            // start at the address of the `ProcessIdList` field.
            let ids = unsafe {
                let count = (*list).NumberOfProcessIdsInList as usize;
                std::slice::from_raw_parts(
                    ptr::addr_of!((*list).ProcessIdList).cast::<usize>(),
                    count,
                )
            };
            return Ok(ids
                .iter()
                .map(|&id| u32::try_from(id).expect("a process id fits a u32"))
                .collect());
        }
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

impl AsRawHandle for Job {
    fn as_raw_handle(&self) -> RawHandle {
        self.raw()
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
