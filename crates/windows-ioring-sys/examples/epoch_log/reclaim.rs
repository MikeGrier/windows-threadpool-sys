// Copyright (c) 2026 Mike Grier
//! Reclamation: a non-ring operation ordered against ring epochs (M14.1).
//!
//! # Why this file exists
//!
//! Once epoch *N* is durable, the log prefix it covers can be reclaimed. That
//! reclamation is an `FSCTL` -- `FSCTL_SET_ZERO_DATA` -- and the ring cannot
//! express it: `IoRing`'s operation table is seven entries, none of them an
//! ioctl. So the log has an operation that **must** happen after a ring
//! operation, and no ring mechanism can order it.
//!
//! `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` is specifically not the answer. It orders
//! SQEs against SQEs (D-24) and is powerless in both directions across the
//! ring boundary: it can neither make a ring op wait for this `FSCTL` nor make
//! this `FSCTL` wait for ring ops. This is the case that forced the whole
//! `completion_event` design -- the consumer conversation behind M11 started
//! here.
//!
//! The ordering is therefore enforced **by the log, in its own code**: the
//! reclaim is not issued until the commit that makes the epoch durable has
//! been observed. That is the entire technique, and it is deliberately
//! unglamorous. What the ring provides is not the ordering but the ability to
//! *wait for both kinds of completion in one place* without a blocking drain,
//! which is what [`crate::event_loop`] does.
//!
//! # Why a worker thread
//!
//! `windows-overlapped-io-sys`'s `BlockingEndpoint` completes an operation
//! synchronously, inside `GetOverlappedResult`. That is the supported shape
//! for that backend ("one owner issuing operations in sequence"), and it is
//! the right shape for a control-plane operation -- but the log's event loop
//! must not block in it, or ring completions stop being serviced for the
//! duration.
//!
//! So the endpoint is owned by a worker thread, and the log learns of
//! completion the same way it learns of anything else: an event it is already
//! waiting on. Bridging a blocking API into an event loop with a thread and an
//! event is ordinary, and worth showing precisely because it is what a real
//! consumer has to do when a dependency hands back a blocking completion.
//!
//! # An honest note on what the reclaim actually does
//!
//! `FSCTL_SET_ZERO_DATA` zeroes a range. On a **sparse** file it deallocates
//! the backing clusters, which is real reclamation; on a non-sparse file it
//! writes zeroes and frees nothing. This sample marks the log sparse first, so
//! the call means what its name suggests -- but it does not verify the volume
//! honoured it, and it does not depend on that for anything it asserts.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::sync::mpsc;

use windows_overlapped_io_sys::{BlockingEndpoint, UnassociatedEndpoint};
use windows_sys::Win32::System::Ioctl::{
    FILE_ZERO_DATA_INFORMATION, FSCTL_SET_SPARSE, FSCTL_SET_ZERO_DATA,
};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

use crate::commit::Epoch;

/// A request to reclaim the log's bytes below `end_offset`.
struct Request {
    epoch: Epoch,
    end_offset: u64,
}

/// What a finished reclamation reported.
pub struct Reclaimed {
    pub epoch: Epoch,
    pub bytes: u64,
    pub result: io::Result<()>,
}

/// Runs `FSCTL_SET_ZERO_DATA` off the event-loop thread and signals an event
/// the log is already waiting on.
pub struct Reclaimer {
    requests: Option<mpsc::Sender<Request>>,
    done: OwnedHandle,
    results: mpsc::Receiver<Reclaimed>,
    worker: Option<std::thread::JoinHandle<()>>,
    in_flight: bool,
}

impl Reclaimer {
    /// Open a second handle to `path` for control operations and start the
    /// worker.
    ///
    /// A separate handle from the log's data handle on purpose: the data
    /// handle belongs to the ring, and an overlapped endpoint wants ownership
    /// of what it drives.
    ///
    /// # Errors
    ///
    /// Any error from opening the endpoint, creating the completion event, or
    /// marking the file sparse.
    pub fn new(path: &Path) -> io::Result<Self> {
        let endpoint = UnassociatedEndpoint::open(path, true, true, 0)?;
        let mut endpoint = BlockingEndpoint::new(endpoint)
            .map_err(|_| io::Error::other("the endpoint suppresses its own completion event"))?;

        // Sparse first, or SET_ZERO_DATA writes zeroes instead of freeing
        // clusters. Best-effort: a volume that refuses leaves the reclaim
        // meaningful as a demonstration but not as a space saving, and the
        // sample asserts nothing about the space.
        // SAFETY: `FSCTL_SET_SPARSE` with an empty input is self-contained --
        // it embeds no pointers -- which is what this seam requires.
        let _ = unsafe { endpoint.ioctl(FSCTL_SET_SPARSE, &[], &mut []) };

        let done = event(false)?;
        let signal = done.try_clone()?;
        let (requests, request_rx) = mpsc::channel::<Request>();
        let (result_tx, results) = mpsc::channel::<Reclaimed>();

        let worker = std::thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let result = zero_range(&mut endpoint, request.end_offset);
                let _ = result_tx.send(Reclaimed {
                    epoch: request.epoch,
                    bytes: request.end_offset,
                    result,
                });
                // Signal only after the result is queued, so a wake always has
                // something to collect -- the same ordering rule the ring's own
                // completion event follows.
                // SAFETY: `signal` is a live event handle owned by this thread.
                unsafe { SetEvent(signal.as_raw_handle()) };
            }
        });

        Ok(Self {
            requests: Some(requests),
            done,
            results,
            worker: Some(worker),
            in_flight: false,
        })
    }

    /// The handle the log's multiplexed wait watches.
    pub fn completion_handle(&self) -> &OwnedHandle {
        &self.done
    }

    /// Whether a reclamation is outstanding.
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    /// Ask the worker to reclaim everything below `end_offset`, on behalf of
    /// `epoch`.
    ///
    /// **Call only after `epoch`'s commit completion has been observed.** That
    /// is the ordering the ring cannot enforce and this log must: reclaiming a
    /// prefix whose durability has not been established would discard data the
    /// log has not yet promised is anywhere else.
    ///
    /// # Errors
    ///
    /// Fails if a reclamation is already outstanding, or if the worker has
    /// stopped.
    pub fn request(&mut self, epoch: Epoch, end_offset: u64) -> io::Result<()> {
        if self.in_flight {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "a reclamation is already outstanding",
            ));
        }
        let requests = self
            .requests
            .as_ref()
            .ok_or_else(|| io::Error::other("the reclaimer is shutting down"))?;
        requests
            .send(Request { epoch, end_offset })
            .map_err(|_| io::Error::other("the reclaim worker has stopped"))?;
        self.in_flight = true;
        Ok(())
    }

    /// Collect a finished reclamation, if one is ready.
    pub fn take_completed(&mut self) -> Option<Reclaimed> {
        match self.results.try_recv() {
            Ok(reclaimed) => {
                self.in_flight = false;
                Some(reclaimed)
            }
            Err(_) => None,
        }
    }
}

impl Drop for Reclaimer {
    fn drop(&mut self) {
        // Dropping the sender ends the worker's `recv` loop; joining then
        // guarantees the endpoint is closed before this returns.
        self.requests = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Zero (and, on a sparse file, deallocate) `[0, end)`.
fn zero_range(endpoint: &mut BlockingEndpoint, end: u64) -> io::Result<()> {
    let info = FILE_ZERO_DATA_INFORMATION {
        FileOffset: 0,
        BeyondFinalZero: i64::try_from(end).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "reclaim range exceeds i64::MAX",
            )
        })?,
    };
    // SAFETY: `FILE_ZERO_DATA_INFORMATION` is two `i64`s and embeds no
    // pointers, so `FSCTL_SET_ZERO_DATA`'s input is self-contained -- the
    // condition this seam's safety contract names. The byte view below borrows
    // `info`, which outlives the blocking call.
    let input = unsafe {
        std::slice::from_raw_parts(
            std::ptr::from_ref(&info).cast::<u8>(),
            std::mem::size_of::<FILE_ZERO_DATA_INFORMATION>(),
        )
    };
    // SAFETY: as above; the control code takes no output buffer.
    unsafe { endpoint.ioctl(FSCTL_SET_ZERO_DATA, input, &mut []) }?;
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
