// Copyright (c) 2026 Mike Grier
//! Asynchronous procedure calls: `QueueUserAPC`.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, RawHandle};
use std::ptr;

use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThread, GetProcessIdOfThread, QueueUserAPC,
};

/// What is queued: a routine to run once, on the thread the APC is queued to.
type Routine = Box<dyn FnOnce() + Send + 'static>;

/// Queue `routine` to run on `thread` the next time it waits alertably: `QueueUserAPC`.
///
/// What ends an alertable wait ([`Alertable::Yes`](crate::Alertable::Yes)) is an APC, and this is
/// how one is made. The routine runs **on `thread`**, inside whichever alertable wait or sleep it
/// next makes, before that wait returns [`WaitOutcome::Alerted`](crate::WaitOutcome::Alerted); a
/// thread that never waits alertably never runs it. APCs queued to a thread run in the order they
/// were queued.
///
/// `thread` is any handle that lends itself, and must carry `THREAD_SET_CONTEXT` access, to queue,
/// and `THREAD_QUERY_LIMITED_INFORMATION`, to ask which process it belongs to: a
/// [`std::thread::JoinHandle`] carries both.
///
/// **It must be a thread of this process.** The routine is an address and a closure in this
/// process, which mean nothing in another, where delivering it would run whatever is at that
/// address. A thread of another process is refused, as [`io::ErrorKind::InvalidInput`], before
/// anything is queued.
///
/// **A thread that has not finished starting runs it as it starts.** Windows delivers an APC queued
/// to a new thread when the thread begins, before it reaches the Rust runtime that
/// [`std::thread::spawn`] sets up around the closure it was given, so the routine runs outside that
/// runtime: `std::thread::current` there aborts the process ("current thread handle already
/// set"). Queue to a thread known to be running -- one that has announced itself -- unless the
/// routine touches nothing of the runtime.
///
/// **The routine may never run.** If the thread ends first, or never waits alertably, it stays
/// queued and is not dropped, so what it captured is leaked; no error says so, because the call
/// succeeded. A routine that must not be lost needs a thread known to wait alertably.
///
/// **It runs in whatever context the thread is in** when it makes the wait -- possibly while the
/// thread holds locks or is inside a callback of the caller's -- so it must not assume the thread
/// is idle. A panic in it aborts the process, as a panic out of any `extern "system"` function does.
///
/// # Errors
///
/// Invalid handle for one that is not a thread, access denied for a thread handle without
/// `THREAD_QUERY_LIMITED_INFORMATION` or `THREAD_SET_CONTEXT`, [`io::ErrorKind::InvalidInput`] for a
/// thread of another process, and otherwise the error from `QueueUserAPC`. The routine is dropped,
/// unrun, when the call fails.
pub fn queue_user_apc(
    thread: impl AsHandle,
    routine: impl FnOnce() + Send + 'static,
) -> io::Result<()> {
    let thread = thread.as_handle().as_raw_handle();
    require_this_process(thread)?;
    queue(thread, Box::new(routine))
}

/// Refuse a thread that is not this process's: a routine's address is only meaningful here.
fn require_this_process(thread: RawHandle) -> io::Result<()> {
    // SAFETY: `thread` is open for the call, borrowed by the caller; a handle that is not a thread
    // makes the call fail, reported below.
    let owner = unsafe { GetProcessIdOfThread(thread) };
    if owner == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `GetCurrentProcessId` takes no arguments.
    if owner != unsafe { GetCurrentProcessId() } {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the thread belongs to another process, where this process's routine means nothing",
        ));
    }
    Ok(())
}

/// [`queue_user_apc`] to the calling thread.
///
/// The routine runs in the next alertable wait the thread makes, which is what makes the wait's
/// [`Alerted`](crate::WaitOutcome::Alerted) ending reachable without a second thread.
///
/// # Errors
///
/// The error from `QueueUserAPC`, which for the calling thread arises only when the kernel cannot
/// allocate the APC; no test reaches it.
pub fn queue_user_apc_to_current_thread(routine: impl FnOnce() + Send + 'static) -> io::Result<()> {
    // SAFETY: `GetCurrentThread` takes no arguments and returns a pseudo-handle for the calling
    // thread, valid on that thread without being closed.
    let thread = unsafe { GetCurrentThread() };
    queue(thread, Box::new(routine))
}

fn queue(thread: RawHandle, routine: Routine) -> io::Result<()> {
    // The routine is a fat pointer and the call carries one machine word, so it is boxed again.
    let data = Box::into_raw(Box::new(routine));
    // SAFETY: `run` is a valid APC routine, `thread` is open for the call (borrowed by the caller,
    // or the pseudo-handle), and the word is a pointer that `run` takes back exactly once.
    let queued = unsafe { QueueUserAPC(Some(run), thread, data.expose_provenance()) };
    if queued == 0 {
        let error = io::Error::last_os_error();
        // SAFETY: the call failed, so nothing was queued and the pointer is still ours alone.
        drop(unsafe { Box::from_raw(data) });
        return Err(error);
    }
    Ok(())
}

/// The APC routine: takes back what `queue` boxed, and runs it.
///
/// # Safety
///
/// `data` must be the pointer `queue` gave to a successful `QueueUserAPC`, which the kernel
/// delivers at most once.
unsafe extern "system" fn run(data: usize) {
    let data = ptr::with_exposed_provenance_mut::<Routine>(data);
    // SAFETY: by the contract above, `data` is the live box `queue` made, taken back here once.
    let routine = unsafe { Box::from_raw(data) };
    (*routine)();
}

#[cfg(test)]
mod tests;
