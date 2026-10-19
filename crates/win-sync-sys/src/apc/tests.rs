// Copyright (c) 2026 Mike Grier
//! Unit tests for `queue_user_apc` and `queue_user_apc_to_current_thread`.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, ThreadId};

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SET_CONTEXT, TerminateProcess,
};

use crate::test_support::{BOUND, DropCount, manual};
use crate::{
    Access, Alertable, WaitOutcome, duplicate_handle_with_access, queue_user_apc,
    queue_user_apc_to_current_thread, wait_for_single_object_ex,
};

/// An alertable wait on an event nothing signals, which an APC is the only way to end.
fn alertable_wait(timeout: std::time::Duration) -> WaitOutcome {
    let never = manual(false);
    wait_for_single_object_ex(&never, Some(timeout), Alertable::Yes).expect("the wait is answered")
}

/// Starts a thread running `body`, and returns once the thread is running it.
///
/// An APC queued to a thread that has not started runs as the thread starts, before the Rust
/// runtime has set the thread up, so a test that queues one waits for the thread to be running
/// first. The documentation of `queue_user_apc` says why.
fn spawn_running<T: Send + 'static>(
    body: impl FnOnce() -> T + Send + 'static,
) -> thread::JoinHandle<T> {
    let (started, running) = mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        started.send(()).expect("announce");
        body()
    });
    running.recv_timeout(BOUND).expect("the thread is running");
    handle
}

#[test]
fn an_apc_queued_to_the_current_thread_runs_in_its_next_alertable_wait() {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    queue_user_apc_to_current_thread(move || flag.store(true, Ordering::SeqCst)).expect("queue");
    assert!(!ran.load(Ordering::SeqCst), "queueing does not run it");

    assert_eq!(alertable_wait(BOUND), WaitOutcome::Alerted);
    assert!(
        ran.load(Ordering::SeqCst),
        "it ran before the wait returned"
    );
}

#[test]
fn a_wait_that_is_not_alertable_leaves_the_apc_queued() {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    queue_user_apc_to_current_thread(move || flag.store(true, Ordering::SeqCst)).expect("queue");

    let never = manual(false);
    let outcome = wait_for_single_object_ex(&never, Some(std::time::Duration::ZERO), Alertable::No)
        .expect("wait");
    assert_eq!(outcome, WaitOutcome::TimedOut);
    assert!(!ran.load(Ordering::SeqCst), "not alertable, so not run");

    assert_eq!(alertable_wait(BOUND), WaitOutcome::Alerted);
    assert!(ran.load(Ordering::SeqCst), "the next alertable wait ran it");
}

#[test]
fn an_apc_runs_exactly_once() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    queue_user_apc_to_current_thread(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .expect("queue");

    assert_eq!(alertable_wait(BOUND), WaitOutcome::Alerted);
    assert_eq!(
        alertable_wait(std::time::Duration::ZERO),
        WaitOutcome::TimedOut,
        "nothing is left to end a second wait"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn apcs_queued_to_a_thread_run_in_the_order_they_were_queued() {
    let order = Arc::new(Mutex::new(Vec::new()));
    for index in 0..8 {
        let order = Arc::clone(&order);
        queue_user_apc_to_current_thread(move || order.lock().expect("lock").push(index))
            .expect("queue");
    }
    assert_eq!(alertable_wait(BOUND), WaitOutcome::Alerted);
    let mut seen = order.lock().expect("lock").clone();
    // Whatever one wait ran, the next runs the rest, so the order across waits is what is checked.
    while seen.len() < 8 {
        assert_eq!(alertable_wait(BOUND), WaitOutcome::Alerted);
        seen = order.lock().expect("lock").clone();
    }
    assert_eq!(seen, (0..8).collect::<Vec<_>>());
}

#[test]
fn an_apc_runs_on_the_thread_it_was_queued_to_not_the_one_that_queued_it() {
    let (ids, received) = mpsc::channel::<ThreadId>();
    let worker = spawn_running(move || {
        assert_eq!(alertable_wait(BOUND), WaitOutcome::Alerted);
        thread::current().id()
    });
    queue_user_apc(&worker, move || {
        ids.send(thread::current().id()).expect("send");
    })
    .expect("queue to the worker");

    let ran_on = received.recv_timeout(BOUND).expect("the apc ran");
    let worker_id = worker.join().expect("the worker");
    assert_eq!(ran_on, worker_id);
    assert_ne!(ran_on, thread::current().id());
}

#[test]
fn what_the_routine_captured_moves_with_it() {
    let (send, receive) = mpsc::channel::<String>();
    let owned = String::from("moved into the apc");
    queue_user_apc_to_current_thread(move || send.send(owned).expect("send")).expect("queue");
    assert_eq!(alertable_wait(BOUND), WaitOutcome::Alerted);
    assert_eq!(receive.try_recv().expect("sent"), "moved into the apc");
}

#[test]
fn a_handle_that_is_not_a_thread_is_refused_and_the_routine_is_dropped_unrun() {
    let event = manual(false);
    let ran = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let (flag, guard) = (Arc::clone(&ran), DropCount(Arc::clone(&drops)));

    let error = queue_user_apc(&event, move || {
        let _guard = &guard;
        flag.store(true, Ordering::SeqCst);
    })
    .expect_err("an event is not a thread");

    assert_eq!(
        error.raw_os_error(),
        i32::try_from(ERROR_INVALID_HANDLE).ok()
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1, "dropped, not leaked");
    assert!(!ran.load(Ordering::SeqCst));
}

#[test]
fn a_thread_of_another_process_is_refused_and_the_routine_is_dropped_unrun() {
    // A suspended child, whose main thread handle carries every access and belongs to another
    // process: delivering this process's routine there would run whatever is at its address.
    let mut command: Vec<u16> = "cmd.exe /c exit\0".encode_utf16().collect();
    // SAFETY: all-zero is a valid STARTUPINFOW and PROCESS_INFORMATION before they are filled in.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = u32::try_from(size_of::<STARTUPINFOW>()).expect("a small struct");
    let mut child: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `command` is writable and NUL-terminated, the pointers that may be null are, and
    // `startup` and `child` are live for the call.
    let created = unsafe {
        CreateProcessW(
            std::ptr::null(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_SUSPENDED | CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut child,
        )
    };
    assert_ne!(
        created,
        0,
        "start a suspended child: {}",
        io::Error::last_os_error()
    );
    // SAFETY: the call succeeded, so both handles are open and ours alone.
    let (thread, process) = unsafe {
        (
            OwnedHandle::from_raw_handle(child.hThread),
            OwnedHandle::from_raw_handle(child.hProcess),
        )
    };

    let ran = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let (flag, guard) = (Arc::clone(&ran), DropCount(Arc::clone(&drops)));
    let result = queue_user_apc(&thread, move || {
        let _guard = &guard;
        flag.store(true, Ordering::SeqCst);
    });
    // SAFETY: `process` is the child's own handle, open until it drops below.
    unsafe { TerminateProcess(process.as_raw_handle(), 1) };

    let error = result.expect_err("a thread of another process");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(drops.load(Ordering::SeqCst), 1, "dropped, not leaked");
    assert!(!ran.load(Ordering::SeqCst));
}

#[test]
fn a_thread_handle_without_set_context_access_is_refused_and_one_with_it_is_not() {
    let (release, hold) = mpsc::channel::<()>();
    let worker = spawn_running(move || {
        // The thread runs APCs only if it waits alertably, so it does once released.
        hold.recv().expect("released");
        alertable_wait(BOUND)
    });

    let ran = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let (flag, guard) = (Arc::clone(&ran), DropCount(Arc::clone(&drops)));
    let without = duplicate_handle_with_access(&worker, Access::SYNCHRONIZE).expect("duplicate");
    // SYNCHRONIZE alone lacks the query access as well, so this one is refused by the process
    // check, before `QueueUserAPC` is called; the next test reaches the call's own refusal.
    let error = queue_user_apc(&without, move || {
        let _guard = &guard;
        flag.store(true, Ordering::SeqCst);
    })
    .expect_err("no THREAD_SET_CONTEXT");
    assert_eq!(
        error.raw_os_error(),
        i32::try_from(ERROR_ACCESS_DENIED).ok()
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1, "dropped, not leaked");

    let with = duplicate_handle_with_access(
        &worker,
        Access::from_bits(THREAD_SET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION)
            | Access::SYNCHRONIZE,
    )
    .expect("duplicate");
    let flag = Arc::clone(&ran);
    queue_user_apc(&with, move || flag.store(true, Ordering::SeqCst)).expect("queue");

    release.send(()).expect("release");
    assert_eq!(worker.join().expect("worker"), WaitOutcome::Alerted);
    assert!(ran.load(Ordering::SeqCst), "the permitted one ran");
}

#[test]
fn a_thread_of_this_process_that_queueing_is_refused_for_is_refused_and_the_routine_is_dropped() {
    // The handle can be asked which process it belongs to (THREAD_QUERY_LIMITED_INFORMATION), so it
    // gets past that check, and lacks THREAD_SET_CONTEXT, so the refusal is `QueueUserAPC`'s own,
    // after the routine was boxed and handed to it: the failure the other refusals come before.
    let (release, hold) = mpsc::channel::<()>();
    let worker = spawn_running(move || hold.recv().expect("released"));

    let ran = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let (flag, guard) = (Arc::clone(&ran), DropCount(Arc::clone(&drops)));
    let query_only =
        duplicate_handle_with_access(&worker, Access::from_bits(THREAD_QUERY_LIMITED_INFORMATION))
            .expect("duplicate");
    let error = queue_user_apc(&query_only, move || {
        let _guard = &guard;
        flag.store(true, Ordering::SeqCst);
    })
    .expect_err("no THREAD_SET_CONTEXT");

    assert_eq!(
        error.raw_os_error(),
        i32::try_from(ERROR_ACCESS_DENIED).ok()
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1, "dropped, not leaked");
    assert!(!ran.load(Ordering::SeqCst));

    release.send(()).expect("release");
    worker.join().expect("worker");
}

#[test]
fn an_apc_for_a_thread_that_never_waits_alertably_is_neither_run_nor_dropped() {
    let (release, hold) = mpsc::channel::<()>();
    let worker = spawn_running(move || hold.recv().expect("released"));

    let ran = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let (flag, guard) = (Arc::clone(&ran), DropCount(Arc::clone(&drops)));
    queue_user_apc(&worker, move || {
        let _guard = &guard;
        flag.store(true, Ordering::SeqCst);
    })
    .expect("queue");

    release.send(()).expect("release");
    worker.join().expect("worker");
    assert!(!ran.load(Ordering::SeqCst), "it never ran");
    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "and what it captured was leaked, as the documentation says"
    );
}
