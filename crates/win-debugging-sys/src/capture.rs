// Copyright (c) 2026 Mike Grier
//! A debugger that runs a program and keeps the strings it sends (WD-D-3), so that a test, here or in
//! another crate, can see that what a crate sends with [`output_debug_string`](crate::output_debug_string)
//! reaches a debugger by the documented route and not by a protocol no page describes.
//!
//! This is a test facility and not a wrapper of the debugging functions: it is behind the `test-util` feature,
//! it starts a process, and the policy it carries (a bound on the wait, the process ended if it is reached, the
//! handles closed) is a test's. It uses `CreateProcessW` with `DEBUG_ONLY_THIS_PROCESS`, and for each event
//! `WaitForDebugEventEx`, `ReadProcessMemory` and `ContinueDebugEvent`. A debugger that waits with
//! `WaitForDebugEventEx` opts into Unicode strings, which the page for `OutputDebugStringW` says a debugger must do
//! to be sent them.

use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::path::Path;
use std::ptr::null;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, DBG_CONTINUE, DBG_EXCEPTION_NOT_HANDLED, HANDLE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CREATE_PROCESS_DEBUG_EVENT, ContinueDebugEvent, DEBUG_EVENT, EXCEPTION_DEBUG_EVENT,
    EXIT_PROCESS_DEBUG_EVENT, LOAD_DLL_DEBUG_EVENT, OUTPUT_DEBUG_STRING_EVENT,
    OUTPUT_DEBUG_STRING_INFO, ReadProcessMemory, WaitForDebugEventEx,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DEBUG_ONLY_THIS_PROCESS, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
    TerminateProcess,
};

/// One string a debugged process sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugString {
    /// Whether it arrived as Unicode, which is `OUTPUT_DEBUG_STRING_INFO`'s `fUnicode` being nonzero, and not as
    /// ANSI.
    pub unicode: bool,
    /// Its units, as UTF-16 whichever way it arrived, up to its first NUL.
    ///
    /// For a string the event could not describe whole this is what the event described: the page for the structure
    /// says its length field holds "the lower 16 bits of the length of the string in bytes".
    pub units: Vec<u16>,
}

impl DebugString {
    /// The units as text, with any unit that is not valid UTF-16 replaced.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf16_lossy(&self.units)
    }
}

/// What a debugger saw of a process it ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captured {
    /// Each string the process sent, in the order it was received.
    pub strings: Vec<DebugString>,
    /// The code the process exited with, or `None` if it had not exited when the bound was reached, in which case it
    /// was ended.
    pub exit_code: Option<u32>,
}

/// Runs `program` with `arguments` as a process being debugged, and returns what it sent to the debugger.
///
/// The command line is `program` in quotes, then each argument after a space as given: an argument that needs
/// quoting is quoted by the caller. The process runs without a console, so what it prints reaches no one. It is
/// waited for up to `bound`; one that has not exited by then is ended, and [`Captured::exit_code`] is `None`.
///
/// # Errors
///
/// The operating system's, if the process could not be created, a debug event could not be continued, or a string
/// could not be read out of the process. A process that was started is ended and its handles closed first.
pub fn run_debugged(program: &Path, arguments: &[&str], bound: Duration) -> io::Result<Captured> {
    let mut command_line = format!("\"{}\"", program.display());
    for argument in arguments {
        command_line.push(' ');
        command_line.push_str(argument);
    }
    let mut command: Vec<u16> = command_line.encode_utf16().chain([0]).collect();

    // SAFETY: both structures are plain data that the call reads or fills in, and all-zero is a valid value of each.
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    // SAFETY: as above.
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    // SAFETY: `command` is a writable, NUL-terminated wide string that outlives the call, as the page requires of
    // the command line; the other pointers are null, which the page allows, or point at the structures above.
    let created = unsafe {
        CreateProcessW(
            null(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            DEBUG_ONLY_THIS_PROCESS | DETACHED_PROCESS,
            null(),
            null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }

    let captured = collect(&process, bound);

    // SAFETY: the handles are the ones the call returned and are closed once. A process still running, because the
    // bound was reached or a call failed, is ended first.
    unsafe {
        if !matches!(
            captured,
            Ok(Captured {
                exit_code: Some(_),
                ..
            })
        ) {
            TerminateProcess(process.hProcess, 1);
        }
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    captured
}

/// `EXCEPTION_BREAKPOINT`, the loader's first event in a debugged process.
const BREAKPOINT: i32 = 0x8000_0003_u32 as i32;

/// The debugger's main loop: waits for each event, keeps the strings, and continues the process.
fn collect(process: &PROCESS_INFORMATION, bound: Duration) -> io::Result<Captured> {
    let started = Instant::now();
    let mut captured = Captured {
        strings: Vec::new(),
        exit_code: None,
    };
    while started.elapsed() < bound && captured.exit_code.is_none() {
        // SAFETY: all-zero is a valid `DEBUG_EVENT`, which the call fills in.
        let mut event: DEBUG_EVENT = unsafe { zeroed() };
        // SAFETY: `event` is valid for the call to write.
        if unsafe { WaitForDebugEventEx(&mut event, 1_000) } == 0 {
            continue;
        }
        let mut status = DBG_CONTINUE;
        // SAFETY: each union member read is the one the event code says the system filled in.
        unsafe {
            match event.dwDebugEventCode {
                OUTPUT_DEBUG_STRING_EVENT => {
                    captured
                        .strings
                        .push(read_string(process.hProcess, &event.u.DebugString)?);
                }
                CREATE_PROCESS_DEBUG_EVENT => close_image(event.u.CreateProcessInfo.hFile),
                LOAD_DLL_DEBUG_EVENT => close_image(event.u.LoadDll.hFile),
                EXIT_PROCESS_DEBUG_EVENT => {
                    captured.exit_code = Some(event.u.ExitProcess.dwExitCode);
                }
                // The loader's breakpoint is continued; any other exception is the process's to handle.
                EXCEPTION_DEBUG_EVENT
                    if event.u.Exception.ExceptionRecord.ExceptionCode != BREAKPOINT =>
                {
                    status = DBG_EXCEPTION_NOT_HANDLED;
                }
                _ => {}
            }
        }
        // SAFETY: the ids are those of the event just received.
        if unsafe { ContinueDebugEvent(event.dwProcessId, event.dwThreadId, status) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(captured)
}

/// Closes the image or DLL handle an event hands the debugger, as the page for the wait asks.
///
/// # Safety
///
/// `file` is the handle from the event, or null.
unsafe fn close_image(file: HANDLE) {
    if !file.is_null() {
        // SAFETY: the caller's: the debugger owns this handle and it is closed once.
        unsafe { CloseHandle(file) };
    }
}

/// Reads the string an output event describes out of the process, by `ReadProcessMemory`, which the page for the
/// structure says is how.
fn read_string(process: HANDLE, info: &OUTPUT_DEBUG_STRING_INFO) -> io::Result<DebugString> {
    let unicode = info.fUnicode != 0;
    // The field holds "the lower 16 bits of the length of the string in bytes".
    let bytes = usize::from(info.nDebugStringLength);
    let mut buffer = vec![0_u8; bytes];
    let mut read = 0_usize;
    // SAFETY: `buffer` is valid for `bytes` writes, and `read` for one.
    let ok = unsafe {
        ReadProcessMemory(
            process,
            info.lpDebugStringData as *const c_void,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut read,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(read);
    let mut units: Vec<u16> = if unicode {
        buffer
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect()
    } else {
        buffer.iter().map(|&byte| u16::from(byte)).collect()
    };
    // The length counts the terminator the call sends; what is kept is the string before it.
    if let Some(end) = units.iter().position(|&unit| unit == 0) {
        units.truncate(end);
    }
    Ok(DebugString { unicode, units })
}
