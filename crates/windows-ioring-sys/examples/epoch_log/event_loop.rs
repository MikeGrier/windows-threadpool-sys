// Copyright (c) 2026 Mike Grier
//! The event loop (M13.4, extended in M14.1): the ring's completion event
//! waited on alongside a non-ring completion event and a shutdown event.
//!
//! This is the shape a real log needs and the one M13.3's fused
//! `submit_and_wait` cannot provide: a log that must also service something
//! that is not ring I/O -- an `FSCTL` here (M14.1), a shutdown signal, a socket
//! or a timer in a real service -- cannot park inside `SubmitIoRing`, because
//! nothing would wake it for the other handle. `IoRing::completion_event` hands
//! back an owned duplicate of the ring's event so the thread can block in
//! `WaitForMultipleObjects` instead, without giving up the ring.
//!
//! Note what this does and does not buy. It does **not** order the non-ring
//! operation against ring operations -- `IOSQE_FLAGS_DRAIN_PRECEDING_OPS`
//! orders SQEs against SQEs (D-24) and has no reach across the boundary, so
//! that ordering stays the log's job (see [`crate::reclaim`]). What it buys is
//! that both kinds of completion can be *awaited in one place*, so enforcing
//! the ordering does not cost a blocking drain.
//!
//! Ownership does not change: this thread still owns, submits to, and drains
//! its ring, so this is Model B with a different wakeup source, not Model A.
//!
//! # D-19, which is why this file is written the way it is
//!
//! The ring's completion event is **edge-triggered on the completion queue
//! going from empty to non-empty**. It is not level-triggered and not one
//! signal per completion: a batch of eight completions arriving together
//! produces exactly one wakeup. Two rules follow, and both are load-bearing:
//!
//! 1. **Drain to empty on every pass.** Not only on the pass the ring woke.
//!    A wait re-entered with entries still queued blocks until some *later*
//!    completion arrives after the queue has been emptied -- which may be
//!    never. That is a lost-wakeup deadlock, not a latency wobble.
//! 2. **A wake with nothing to pop is normal.** `completion_event` produces
//!    one deliberately as it attaches, so a caller that already submitted
//!    cannot miss its backlog.
//!
//! The drain in [`EventLoop::pump`] therefore sits **outside** the match on
//! which handle woke us, and it drains *to empty* rather than popping once.
//!
//! Of those two, measurement says the second is the one with teeth. Replacing
//! the drain-to-empty with a single `try_pop` deadlocks this example outright:
//! the completion queue stops returning to empty, the edge never re-arms, and
//! the next wait blocks until its timeout with the log's work stranded. That
//! is D-19 as a running program.
//!
//! Moving the drain *inside* the ring's arm, by contrast, does **not** break a
//! conformant loop, and it is worth being honest about why rather than
//! claiming a demonstration this file cannot give. On a loop that already
//! obeys rule 1, the shutdown pass has nothing to pop: any completion that
//! arrived signalled the event, so the ring's own handle is what wakes us and
//! the queue is emptied there. The unconditional placement is defensive -- it
//! costs nothing, it covers a completion landing between the wait returning
//! and the drain, and it is what rule 1 literally says -- but this example
//! cannot make it fail, and neither can any loop that is already correct.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_ioring_sys::IoRing;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};

/// Which handle woke the wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Woken {
    /// The ring signalled: at least one completion arrived into an empty
    /// queue. There may be many, or -- per rule 2 -- none left by the time we
    /// look.
    Ring,
    /// A non-ring operation finished. This is the whole reason the log waits
    /// here rather than inside `SubmitIoRing`: the ring has no way to signal
    /// something it did not issue (M14.1).
    NonRing,
    /// The shutdown latch was set.
    Shutdown,
}

/// The ring's completion event, a non-ring completion event, and a shutdown
/// latch, waited on together.
pub struct EventLoop {
    completion: OwnedHandle,
    non_ring: OwnedHandle,
    shutdown: OwnedHandle,
}

impl EventLoop {
    /// Take the ring's completion event and pair it with `non_ring` -- the
    /// handle some operation the ring cannot express signals -- and a fresh
    /// shutdown latch.
    ///
    /// The ring handle is a *duplicate*: the ring keeps its own, so this one
    /// is ours to hold and to drop without the ring ever signalling a closed
    /// handle.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the system does not report
    /// `IORING_FEATURE_SET_COMPLETION_EVENT`, or any error from duplicating
    /// `non_ring` or creating the shutdown event.
    pub fn new(ring: &mut IoRing, non_ring: &OwnedHandle) -> io::Result<Self> {
        let completion = ring.completion_event()?;
        // Manual-reset, because shutdown is a latch rather than a hand-off:
        // once set, every subsequent wait must keep seeing it. The ring's own
        // event is auto-reset instead, since exactly one waiter must consume
        // each edge (D-21).
        let shutdown = event(true)?;
        Ok(Self {
            completion,
            non_ring: non_ring.try_clone()?,
            shutdown,
        })
    }

    /// A handle that sets the shutdown latch, for whatever decides when to
    /// stop.
    ///
    /// # Errors
    ///
    /// Any error from duplicating the handle.
    pub fn shutdown_handle(&self) -> io::Result<OwnedHandle> {
        self.shutdown.try_clone()
    }

    /// Block until either handle fires, then drain the ring to empty.
    ///
    /// `drain` is called on **every** pass, whichever handle woke us, and is
    /// expected to pop until the completion queue is empty. Returns which
    /// handle woke the wait and how many completions the drain observed --
    /// which may be zero, and legitimately so.
    ///
    /// # Errors
    ///
    /// A timeout is reported as [`io::ErrorKind::TimedOut`] rather than
    /// hidden, so a stuck loop fails instead of spinning. Otherwise any error
    /// from `drain`.
    pub fn pump<F>(&self, timeout_ms: u32, mut drain: F) -> io::Result<(Woken, usize)>
    where
        F: FnMut() -> io::Result<usize>,
    {
        let handles: [HANDLE; 3] = [
            self.completion.as_raw_handle(),
            self.non_ring.as_raw_handle(),
            self.shutdown.as_raw_handle(),
        ];
        // SAFETY: all three handles are live and owned by this value, and
        // `handles` holds exactly the three entries the count promises.
        // `bWaitAll = FALSE`, so this returns as soon as any is signalled --
        // and reports the ring first when several are, since it is the lowest
        // index, which is what stops a set shutdown latch from starving
        // completions.
        let result = unsafe { WaitForMultipleObjects(3, handles.as_ptr(), 0, timeout_ms) };
        let woken = if result == WAIT_OBJECT_0 {
            Woken::Ring
        } else if result == WAIT_OBJECT_0 + 1 {
            Woken::NonRing
        } else if result == WAIT_OBJECT_0 + 2 {
            Woken::Shutdown
        } else if result == WAIT_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "no handle in the multiplexed wait signalled",
            ));
        } else {
            return Err(io::Error::last_os_error());
        };

        // RULE 1. Two properties, and they are not equally load-bearing.
        //
        // Draining *to empty* is the one with teeth: pop once instead and the
        // queue never returns to empty, so the edge never re-arms and the next
        // wait blocks forever. Verified by sabotage -- it deadlocks this
        // example until its timeout.
        //
        // Running on *every* pass, including the shutdown one, is defensive:
        // it costs nothing and covers a completion landing between the wait
        // returning and this call. A loop that already drains to empty will
        // find nothing here on a shutdown wake, so this placement cannot be
        // shown failing -- it is here because rule 1 says every pass, not
        // because this example can prove it.
        let popped = drain()?;
        Ok((woken, popped))
    }
}

/// Set the shutdown latch.
///
/// # Errors
///
/// Any error from `SetEvent`.
pub fn signal(event: &OwnedHandle) -> io::Result<()> {
    // SAFETY: `event` is a live event handle owned by the caller.
    if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// An unnamed, initially-unsignalled event.
fn event(manual_reset: bool) -> io::Result<OwnedHandle> {
    // SAFETY: unnamed event with default security attributes; all pointer
    // arguments are null by design.
    let raw = unsafe {
        CreateEventW(
            std::ptr::null(),
            i32::from(manual_reset),
            0,
            std::ptr::null(),
        )
    };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `CreateEventW` just returned a fresh handle nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}
