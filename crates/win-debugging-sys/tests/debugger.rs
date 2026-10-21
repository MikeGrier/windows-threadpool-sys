// Copyright (c) 2026 Mike Grier
//! What a debugger receives (WD-D-2): this test binary starts itself as a process being debugged, runs
//! `a_process_that_sends_strings` in it, and reads each string the process sends with the documented debugger
//! functions. The debugger opts into Unicode by waiting with `WaitForDebugEventEx`, which the page for
//! `OutputDebugStringW` says a debugger must do to be sent Unicode.
//!
//! This is evidence of what was observed, written as a test so that it is observed again: the crate's
//! contract is the call's page, which says the string is displayed "by the debugger", not how it arrives.

#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::null;
use std::time::{Duration, Instant};

use win_debugging_sys::{output_debug_str, output_debug_string};
use windows_sys::Win32::Foundation::{
    CloseHandle, DBG_CONTINUE, DBG_EXCEPTION_NOT_HANDLED, HANDLE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CREATE_PROCESS_DEBUG_EVENT, ContinueDebugEvent, DEBUG_EVENT, EXCEPTION_DEBUG_EVENT,
    EXIT_PROCESS_DEBUG_EVENT, LOAD_DLL_DEBUG_EVENT, OUTPUT_DEBUG_STRING_EVENT, ReadProcessMemory,
    WaitForDebugEventEx,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DEBUG_ONLY_THIS_PROCESS, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
    TerminateProcess,
};

/// The name of the test the process being debugged runs.
const SENDER: &str = "a_process_that_sends_strings";

/// How long the whole exchange may take. Generous: the process starts, sends a handful of strings, and exits.
const BOUND: Duration = Duration::from_secs(30);

/// The length of the string that is too long for the debugger's event to hold whole.
const LONG: usize = 100_000;

/// What the process sends, in order, before the long string.
fn cases() -> Vec<Vec<u16>> {
    let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    vec![
        units("win-debugging-sys: one"),
        Vec::new(),
        units("two words, and punctuation; \u{e9}"),
        units("\u{3042}\u{1f600}"),
        vec![0xd800, 0x61],
    ]
}

/// The string the process sends last, too long for the event to describe whole.
fn long() -> Vec<u16> {
    vec![u16::from(b'L'); LONG]
}

/// Run in the process being debugged: sends the strings. Run on its own, with no debugger, the call does
/// nothing, so this is harmless in an ordinary run of the tests.
#[test]
fn a_process_that_sends_strings() {
    for case in cases() {
        output_debug_string(&case).expect("no NUL");
    }
    // Refused, so nothing of it is sent, not even what comes before the NUL.
    assert!(output_debug_str("cut\0short").is_err());
    assert!(output_debug_string(&[0x61, 0, 0x62]).is_err());
    output_debug_string(&long()).expect("no NUL");
}

/// One string received, as UTF-16 units whether it arrived as ANSI or Unicode.
struct Received {
    unicode: bool,
    units: Vec<u16>,
}

/// What the debugger saw of the process.
struct Observed {
    strings: Vec<Received>,
    exit_code: Option<u32>,
}

/// Starts this test binary running `SENDER` as a process being debugged, and collects what it sends.
fn debug_the_sender() -> Observed {
    let exe = std::env::current_exe().expect("the test binary's path");
    let mut command: Vec<u16> = format!("\"{}\" --exact {SENDER} --test-threads=1", exe.display())
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: both structures are plain data that the call fills in or reads, and all-zero is a valid value of
    // each; `cb` is set as the page requires.
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    // SAFETY: as above.
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    // SAFETY: `command` is a writable, NUL-terminated wide string that outlives the call, as the page requires
    // for the command line; the other pointers are null, which the page allows, or point at the structures above.
    let created = unsafe {
        CreateProcessW(
            null(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            // Detached, so that what the test harness in the process prints does not reach this console.
            DEBUG_ONLY_THIS_PROCESS | DETACHED_PROCESS,
            null(),
            null(),
            &startup,
            &mut process,
        )
    };
    assert_ne!(created, 0, "the debugged process was not created");

    let observed = collect(&process);

    // SAFETY: the handles are the ones the call returned and are closed once. A process that is still running
    // (the bound was reached) is ended first.
    unsafe {
        if observed.exit_code.is_none() {
            TerminateProcess(process.hProcess, 1);
        }
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    observed
}

/// The debugger's main loop: waits for each event, keeps the strings, and continues the process.
fn collect(process: &PROCESS_INFORMATION) -> Observed {
    let started = Instant::now();
    let mut observed = Observed {
        strings: Vec::new(),
        exit_code: None,
    };
    while started.elapsed() < BOUND && observed.exit_code.is_none() {
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
                    let info = event.u.DebugString;
                    observed.strings.push(read_string(process.hProcess, &info));
                }
                CREATE_PROCESS_DEBUG_EVENT => close_image(event.u.CreateProcessInfo.hFile),
                LOAD_DLL_DEBUG_EVENT => close_image(event.u.LoadDll.hFile),
                EXIT_PROCESS_DEBUG_EVENT => {
                    observed.exit_code = Some(event.u.ExitProcess.dwExitCode);
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
        let continued = unsafe { ContinueDebugEvent(event.dwProcessId, event.dwThreadId, status) };
        assert_ne!(continued, 0, "the event was not continued");
    }
    observed
}

/// `EXCEPTION_BREAKPOINT`, the loader's first event in a debugged process.
const BREAKPOINT: i32 = 0x8000_0003_u32 as i32;

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
fn read_string(
    process: HANDLE,
    info: &windows_sys::Win32::System::Diagnostics::Debug::OUTPUT_DEBUG_STRING_INFO,
) -> Received {
    let unicode = info.fUnicode != 0;
    let unit = if unicode { 2 } else { 1 };
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
    assert_ne!(ok, 0, "the string could not be read out of the process");
    buffer.truncate(read - read % unit);
    let mut units: Vec<u16> = if unicode {
        buffer
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect()
    } else {
        buffer.iter().map(|&b| u16::from(b)).collect()
    };
    // The length counts the terminator the call sends; what is kept is the string before it.
    if let Some(end) = units.iter().position(|&u| u == 0) {
        units.truncate(end);
    }
    Received { unicode, units }
}

#[test]
fn a_debugger_receives_each_string_the_process_sends_and_none_that_was_refused() {
    let observed = debug_the_sender();
    assert_eq!(observed.exit_code, Some(0), "the sender did not pass");
    let expected = cases();
    assert_eq!(
        observed.strings.len(),
        expected.len() + 1,
        "one event for each string sent, and none for the two refused"
    );
    // Observed: a debugger that waits with `WaitForDebugEventEx` is sent each string as Unicode, as the page
    // says it will be, with every unit as sent, an unpaired surrogate included.
    for (received, sent) in observed.strings.iter().zip(&expected) {
        assert!(received.unicode, "{sent:?} arrived as ANSI");
        assert_eq!(&received.units, sent);
    }
}

#[test]
fn a_string_too_long_for_the_event_arrives_cut_at_the_length_the_page_says() {
    let observed = debug_the_sender();
    let received = observed.strings.last().expect("the long string's event");
    assert!(received.unicode);
    // The page for the structure: the length field holds "the lower 16 bits of the length of the string in
    // bytes", and the string's length counts the terminator.
    let sent_bytes = (long().len() + 1) * size_of::<u16>();
    let kept_bytes = sent_bytes % (1 << 16);
    assert_eq!(received.units.len() * size_of::<u16>(), kept_bytes);
    assert!(received.units.iter().all(|&unit| unit == u16::from(b'L')));
}
