// Copyright (c) 2026 Mike Grier
//! M11.6: a worked example of **Model B with a multiplexed wakeup** -- one
//! thread that owns its ring, submits to it, and drains it, while blocking on
//! the ring's completion event *alongside* an unrelated shutdown event.
//!
//! This is not a third delivery architecture. It is Model B: this thread is
//! the only one that ever touches the ring, and nothing here shares, locks, or
//! hands it to a pool. The only thing that changes versus the fused
//! `Batch::submit_and_wait` shape is *what the thread blocks on* -- which is
//! the point, because `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` orders SQEs against
//! SQEs only and cannot order ring I/O against anything else. A domain that
//! must also service a shutdown event, a socket, or a timer needs this.
//!
//! # The contract this example exists to demonstrate (D-19)
//!
//! The ring's completion event is **edge-triggered on the completion queue
//! going from empty to non-empty**. It is not level-triggered and it is not
//! one signal per completion: a batch of eight completions arriving at once
//! produces exactly one wakeup. That is measured rather than documented by
//! Win32, and assuming otherwise hangs rather than merely slows. Two rules
//! follow, and both are load-bearing here:
//!
//! 1. **Drain to empty on every pass** -- not only on the pass the ring's own
//!    handle woke. A wait re-entered with entries still queued blocks until
//!    some later completion arrives *after* the queue has been emptied, which
//!    may be never. That is a lost-wakeup deadlock. In the loop below the
//!    drain deliberately sits outside the `match`, so the shutdown pass drains
//!    too.
//! 2. **A wake with nothing to pop is normal**, never an error.
//!    `IoRing::completion_event` deliberately produces one as it attaches, so
//!    a caller that had already submitted cannot miss its backlog. This
//!    example counts them and reports the total rather than treating any of
//!    them as spurious.
//!
//! Shutdown *with I/O still in flight* is the other half of the shape, and it
//! is why the loop is followed by a quiesce rather than by a `return`: the
//! kernel may still be writing through buffers those tokens own, so the ring
//! cannot close until every outstanding operation has completed. Whether a
//! given run actually exits with work in flight is a timing question -- the
//! ring is the lower wait index and so is serviced first whenever both
//! handles are ready -- so the example reports what happened rather than
//! asserting either outcome.

use std::collections::HashMap;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::mpsc;

use windows_ioring_sys::{Batch, IoRing, PushOptions, Token};
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};

const WAVES: usize = 3;
const CHUNKS: usize = 8;
const CHUNK_LEN: usize = 4096;

/// Generous: the loop should never sit here, so reaching it means something is
/// wrong. A finite bound turns that into an error instead of a hung example.
const WAIT_TIMEOUT_MS: u32 = 10_000;

/// Bound on the shutdown quiesce, for the same reason.
const QUIESCE_TIMEOUT_MS: u32 = 5_000;
const QUIESCE_ATTEMPTS: usize = 64;

/// Where this example's progress reports go, kept as one seam (the repo's
/// architectural pre-step) rather than scattering `println!` across the file.
struct Output<O>(O);

impl<O: io::Write> Output<O> {
    fn report(&mut self, message: &str) {
        let _ = writeln!(self.0, "{message}");
    }
}

/// Which handle woke the multiplexed wait.
enum Woken {
    Ring,
    Shutdown,
}

/// An unnamed event. `manual_reset` makes it a latch that stays signalled
/// once set, which is what a shutdown flag wants; the ring's own completion
/// event is auto-reset instead, because exactly one waiter must consume each
/// edge (D-21).
fn event(manual_reset: bool) -> io::Result<OwnedHandle> {
    // SAFETY: unnamed event with default security attributes, initially
    // unsignalled; all pointer arguments are null by design.
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
    // SAFETY: `CreateEventW` just returned a fresh handle nothing else owns,
    // so `OwnedHandle` becomes its sole owner.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn signal(event: &OwnedHandle) -> io::Result<()> {
    // SAFETY: `event` is a live event handle this example owns.
    if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Block until either handle fires. This is the whole difference from the
/// fused shape: the thread parks here rather than inside `SubmitIoRing`.
fn wait_either(
    completion_event: &OwnedHandle,
    shutdown: &OwnedHandle,
    timeout_ms: u32,
) -> io::Result<Woken> {
    let handles: [HANDLE; 2] = [completion_event.as_raw_handle(), shutdown.as_raw_handle()];
    // SAFETY: both handles are live and owned by this example, and `handles`
    // holds exactly the two entries the count promises. `bWaitAll = FALSE`, so
    // this returns as soon as either is signalled -- and reports the ring
    // first when both are, since it is the lower index, so completions are
    // never starved by a set shutdown latch.
    let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout_ms) };
    if result == WAIT_OBJECT_0 {
        Ok(Woken::Ring)
    } else if result == WAIT_OBJECT_0 + 1 {
        Ok(Woken::Shutdown)
    } else if result == WAIT_TIMEOUT {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "neither the ring's completion event nor the shutdown event signalled",
        ))
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Rule 1's drain: pop until `try_pop` yields `None`, claiming each token so
/// its buffer is recovered rather than leaked.
fn drain(ring: &mut IoRing, pending: &mut HashMap<usize, Token<Vec<u8>>>) -> io::Result<usize> {
    let mut popped = 0;
    while let Some(completion) = ring.try_pop()? {
        let transferred = completion.result()?;
        assert_eq!(transferred, CHUNK_LEN, "each read fills its whole chunk");
        let token = pending
            .remove(&completion.user_data())
            .expect("completion matches a held token");
        let _buffer = token
            .claim_if(&completion)
            .expect("a token claims its own completion");
        popped += 1;
    }
    Ok(popped)
}

/// Queue a wave of reads and submit without waiting.
fn submit_wave(
    ring: &mut IoRing,
    file: RawHandle,
    pending: &mut HashMap<usize, Token<Vec<u8>>>,
) -> io::Result<()> {
    let mut batch = Batch::new(ring);
    for chunk_index in 0..CHUNKS {
        let buffer = vec![0_u8; CHUNK_LEN];
        let offset = (chunk_index * CHUNK_LEN) as u64;
        // SAFETY: `file` stays open for the whole example, and every token is
        // held in `pending` until its own completion has been popped, so the
        // kernel is never writing through a buffer that has been freed.
        let token = unsafe { batch.read_raw(file, buffer, offset, PushOptions::new()) }?;
        pending.insert(token.id(), token);
    }
    // `wait_operations = 0`: submit and return. This thread's blocking point
    // is the multiplexed wait, not here.
    batch.submit_and_wait(0, 0)?;
    Ok(())
}

fn main() -> io::Result<()> {
    let mut output = Output(io::stdout());
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-model-b-multiplexed-{}.tmp",
        std::process::id()
    ));
    let content: Vec<u8> = (0..CHUNKS * CHUNK_LEN).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &content)?;
    let file = std::fs::File::open(&path)?;
    let handle = file.as_raw_handle();

    // Model B: this thread owns the ring for its entire life.
    let mut ring = IoRing::new(64, 64)?;

    // The multiplexed wakeup source. The ring creates and keeps its own event
    // and hands back a *duplicate*, so this handle is ours to hold and to
    // drop for as long as we like -- closing it cannot leave the ring
    // signalling a dead handle, and the ring keeps signalling its own copy.
    let completion_event = ring.completion_event()?;

    // The unrelated handle. Ordering ring I/O against anything non-ring is
    // exactly what the barrier flag cannot do, which is why it is here.
    let shutdown = event(true)?;

    // Something outside the I/O loop decides when to stop -- the reason a
    // second handle is in the wait at all.
    let shutdown_for_control = shutdown.try_clone()?;
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let control = std::thread::spawn(move || {
        if stop_rx.recv().is_ok() {
            let _ = signal(&shutdown_for_control);
        }
    });
    let mut stop_tx = Some(stop_tx);

    let mut pending: HashMap<usize, Token<Vec<u8>>> = HashMap::new();
    let mut waves_submitted = 0_usize;
    let mut completed = 0_usize;
    let mut empty_wakes = 0_usize;

    submit_wave(&mut ring, handle, &mut pending)?;
    waves_submitted += 1;
    output.report("wave 0 submitted");

    loop {
        let woken = wait_either(&completion_event, &shutdown, WAIT_TIMEOUT_MS)?;

        // Rule 1: the drain sits outside the match, so it runs on *every*
        // pass -- including the one the shutdown handle woke. Moving it into
        // a `Woken::Ring` arm is the bug this example exists to prevent,
        // because a pass that returns to the wait without draining to empty
        // can never be woken again: the edge only re-arms when the queue goes
        // empty first.
        let popped = drain(&mut ring, &mut pending)?;
        completed += popped;

        // Rule 2: a wake with nothing to pop is normal. At minimum the setup
        // signal `completion_event` raises produces one.
        if popped == 0 {
            empty_wakes += 1;
        }

        if matches!(woken, Woken::Shutdown) {
            output.report(&format!(
                "shutdown woke the wait; leaving the loop with {} operation(s) still in flight",
                pending.len()
            ));
            break;
        }

        if !pending.is_empty() {
            continue;
        }

        if waves_submitted < WAVES {
            submit_wave(&mut ring, handle, &mut pending)?;
            waves_submitted += 1;
            output.report(&format!("wave {} submitted", waves_submitted - 1));

            if waves_submitted == WAVES {
                // Ask for shutdown *while this wave is outstanding*, because
                // that is when it arrives in a real service -- never at a
                // conveniently quiescent moment.
                //
                // In practice this run will still exit with nothing in
                // flight, and that is worth understanding rather than
                // engineering around: `WaitForMultipleObjects` reports the
                // *lowest* signalled index, the ring is index 0, so whenever
                // both are ready the ring is serviced first. A set shutdown
                // latch therefore cannot starve completions. The quiesce
                // below still has to exist, because "normally" is not
                // "never": if the latch is set during a window where the
                // completion queue happens to be empty and operations are
                // still outstanding, the loop exits with work in flight. The
                // report at the end prints what actually happened on this
                // run rather than assuming either outcome.
                if let Some(stop) = stop_tx.take() {
                    let _ = stop.send(());
                    output.report("shutdown requested while the final wave is outstanding");
                }
            }
        }
    }

    // Dropping the sender releases the control thread even if we left the loop
    // before ever asking it to stop.
    drop(stop_tx);
    control.join().expect("control thread");

    // Shutdown with operations still in flight is the normal case, not an
    // error: the kernel may still be writing through buffers those tokens own,
    // so the ring must not close until they finish. Every SQE that queued
    // produces exactly one completion, so this terminates.
    //
    // Note the wakeup source changes here. The multiplexed loop is over, so
    // the fused submit-and-wait is the right tool for the quiesce -- with no
    // entries queued, its only effect is to block until completions arrive.
    // Both are Model B; only what the thread blocks on differs.
    let mut attempts = 0;
    let mut quiesced = 0_usize;
    while !pending.is_empty() {
        attempts += 1;
        if attempts > QUIESCE_ATTEMPTS {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "outstanding operations never completed during shutdown quiesce",
            ));
        }
        let outstanding = u32::try_from(pending.len()).unwrap_or(u32::MAX);
        Batch::new(&mut ring).submit_and_wait(outstanding, QUIESCE_TIMEOUT_MS)?;
        let popped = drain(&mut ring, &mut pending)?;
        completed += popped;
        quiesced += popped;
    }

    output.report(&format!(
        "quiesce recovered {quiesced} completion(s) after the loop had already exited"
    ));
    output.report(&format!(
        "{completed} reads completed across {waves_submitted} wave(s) on a ring this thread never gave up"
    ));
    output.report(&format!(
        "{empty_wakes} wake(s) had nothing to pop, which the contract requires a caller to tolerate"
    ));

    drop(ring);
    let _ = std::fs::remove_file(&path);
    Ok(())
}
