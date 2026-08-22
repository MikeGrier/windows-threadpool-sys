// Copyright (c) 2026 Mike Grier
//! A callback that panics aborts the process.
//!
//! The crate's callback contract requires that a callback not unwind across the
//! FFI boundary. Nothing contains a violation of that: a panic unwinds to the
//! `extern "system"` trampoline, where Rust turns an escaping unwind into an
//! abort. The panic message still reaches stderr, because the default panic hook
//! runs before unwinding begins -- only the process's survival is given up.
//!
//! This cannot be asserted in-process, because the abort would take the test
//! runner with it. Each test therefore re-executes *this same binary* as a child,
//! selected by an environment variable, and asserts from the parent that the
//! child died abnormally instead of exiting cleanly. The child bodies are plain
//! functions rather than `#[test]`s so the harness never runs them directly.

#![cfg(windows)]

use std::os::windows::io::AsRawHandle;
use std::process::{Command, Stdio};
use std::time::Duration;

use windows_threadpool_sys::io::{IoCompletion, ThreadpoolIo};
use windows_threadpool_sys::timer::ThreadpoolTimer;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
use windows_threadpool_sys::work::ThreadpoolWork;

use windows_overlapped_io_sys::{Issued, Operation, Submitted, UnassociatedEndpoint};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::Threading::SetEvent;

/// The variable naming which child scenario to run. Absent in the parent.
const SCENARIO_VAR: &str = "WTPS_ABORT_SCENARIO";

/// How long a child gets to reach its abort before the parent gives up on it.
const CHILD_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a child that cannot deterministically join its callback waits before
/// concluding the callback will not fire. Only reached when the abort did *not*
/// happen, so it costs nothing on a passing run.
const CHILD_LINGER: Duration = Duration::from_secs(5);

/// Run `scenario` in a child copy of this binary and assert it aborted.
///
/// "Aborted" is asserted as *did not exit successfully* rather than as a specific
/// status. The exact code Windows reports for a Rust abort is an implementation
/// detail of the toolchain (it has changed between releases), so pinning it would
/// make this test fail on a toolchain change that is not a regression in this
/// crate. A clean exit is the only outcome that would mean the panic was
/// contained, which is precisely what must no longer happen.
fn assert_child_aborts(scenario: &str) {
    let exe = std::env::current_exe().expect("locate the test binary");
    let mut child = Command::new(exe)
        .env(SCENARIO_VAR, scenario)
        // The harness would otherwise run every plain fn in this binary as its
        // own libtest thread; the child scenarios are plain fns precisely so the
        // *parent's* run never executes them, but a child that somehow saw more
        // than one still must not race several scenarios concurrently.
        .env("RUST_TEST_THREADS", "1")
        // The child's panic message is expected output, not a failure; keep it
        // out of the parent's stderr so a passing run stays readable.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the child");

    let deadline = std::time::Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the child") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            // A child that neither aborted nor exited is still running: kill and
            // reap it before failing, or it survives this test as an orphan that
            // can hang or contaminate the rest of the CI run.
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the {scenario} child neither aborted nor exited within {CHILD_TIMEOUT:?}; \
                 the panic was probably contained, or the callback never ran"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(
        !status.success(),
        "the {scenario} child exited cleanly, so its callback panic was contained"
    );
}

// --- child scenarios ---

/// Panic from a work callback.
fn child_work_panics() -> ! {
    let work = ThreadpoolWork::new(|| panic!("work callback panics on purpose"), None)
        .expect("create work");
    work.submit();
    work.wait();
    // Only reached if the panic was contained, which is the failure this test
    // exists to catch. Exit cleanly so the parent's assertion fires.
    std::process::exit(0);
}

/// Panic from a timer callback.
fn child_timer_panics() -> ! {
    let timer = ThreadpoolTimer::new(|_firing| panic!("timer callback panics on purpose"), None)
        .expect("create timer");
    timer.set_after(Duration::from_millis(1));
    // Not `timer.wait()`: that waits only for callbacks already *executing*, so
    // with a 1ms delay it returns before the callback has started and the child
    // would exit cleanly without ever panicking. Outliving the firing is what
    // makes the abort the outcome under test.
    std::thread::sleep(CHILD_LINGER);
    std::process::exit(0);
}

/// Panic from a wait callback.
fn child_wait_panics() -> ! {
    let handle = WaitableHandle::event(true, false).expect("create an event");
    let wait = ThreadpoolWait::new(
        handle,
        |_activation| panic!("wait callback panics on purpose"),
        None,
    )
    .expect("create wait");
    wait.arm(None);
    // SAFETY: the wait object owns this event and is armed, so it is live.
    let ok = unsafe { SetEvent(wait.handle().as_raw_handle()) };
    assert_ne!(ok, 0, "SetEvent failed");
    // As above: `wait.wait()` would not block for a callback that has not begun.
    std::thread::sleep(CHILD_LINGER);
    std::process::exit(0);
}

/// Panic from an I/O completion callback.
///
/// This is the one whose containment previously also protected the pool's
/// balanced-start accounting, so it is the most important to prove.
fn child_io_panics() -> ! {
    let path = std::env::temp_dir().join(format!(
        "windows-threadpool-sys-abort-io-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"overlapped").expect("write temp file");

    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    let tp = ThreadpoolIo::new(
        endpoint,
        |_completion: &IoCompletion| panic!("io callback panics on purpose"),
        None,
    )
    .expect("create TP_IO");

    let mut buffer = [0_u8; 32];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;
    let mut operation = Operation::new(());
    operation.set_offset(0);

    // SAFETY: one overlapped ReadFile into `buffer`, which outlives the wait
    // below; the handle is not in skip-on-success mode, so a completion callback
    // is delivered either way.
    let submitted = unsafe {
        tp.submit(operation, |handle, overlapped| {
            let ok = ReadFile(
                handle.as_raw_handle(),
                buf_ptr,
                buf_len,
                std::ptr::null_mut(),
                overlapped,
            );
            if ok != 0 {
                return Ok(Issued::Pending);
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                return Ok(Issued::Pending);
            }
            Err(error)
        })
    };
    assert!(matches!(submitted, Submitted::Pending(_)));

    tp.run_down();
    let _ = std::fs::remove_file(&path);
    std::process::exit(0);
}

/// Dispatch to a child scenario when this binary was spawned as one.
///
/// libtest runs every `#[test]` in the binary, so the dispatch has to happen from
/// inside one of them rather than from a `main`. Each test calls this first; in a
/// child it never returns.
fn dispatch_if_child() {
    let Ok(scenario) = std::env::var(SCENARIO_VAR) else {
        return;
    };
    match scenario.as_str() {
        "work" => child_work_panics(),
        "timer" => child_timer_panics(),
        "wait" => child_wait_panics(),
        "io" => child_io_panics(),
        other => panic!("unknown child scenario {other}"),
    }
}

// --- the assertions ---

#[test]
fn a_panicking_work_callback_aborts_the_process() {
    dispatch_if_child();
    assert_child_aborts("work");
}

#[test]
fn a_panicking_timer_callback_aborts_the_process() {
    dispatch_if_child();
    assert_child_aborts("timer");
}

#[test]
fn a_panicking_wait_callback_aborts_the_process() {
    dispatch_if_child();
    assert_child_aborts("wait");
}

#[test]
fn a_panicking_io_callback_aborts_the_process() {
    dispatch_if_child();
    assert_child_aborts("io");
}
