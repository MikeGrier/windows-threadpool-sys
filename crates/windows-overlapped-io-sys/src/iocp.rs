// Copyright (c) 2026 Mike Grier
//! Raw I/O completion port backend: port ownership, association, and dequeue.
//!
//! A [`CompletionPort`] owns a completion-port handle and can service many
//! endpoints, each associated with a caller-chosen completion key. Association
//! is the consuming transition that binds an [`UnassociatedEndpoint`] to this
//! backend. The port does not create worker threads; the owner decides where
//! [`CompletionPort::get`] runs. Submission of real overlapped operations, and
//! the reclamation that follows their completion, are built on top of this
//! module.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::panic::Location;
use std::sync::{Arc, Mutex, MutexGuard};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::IO::{
    CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
    PostQueuedCompletionStatus,
};

use crate::identity::{OperationId, OperationRegistry};
use crate::{Operation, OperationState, UnassociatedEndpoint};

/// Bound on each `GetQueuedCompletionStatus` wait inside [`CompletionPort::run_down`].
///
/// `run_down` cannot wait without timeout: the port is shareable and
/// [`CompletionPort::get`] takes `&self`, so a concurrent consumer can dequeue
/// the last packet -- and clear the registry entry -- after `run_down` has
/// observed a nonzero outstanding count but before its own wait begins. An
/// unbounded wait would then block forever on a packet that is no longer coming.
/// A bounded wait wakes periodically so the loop can recheck the live count and
/// return once it reaches zero. The interval only bounds that recheck latency; a
/// packet genuinely destined for `run_down` is still returned the instant it
/// arrives, so this is not a busy-poll of a live operation.
const RUN_DOWN_POLL_MS: u32 = 5;

/// Optional per-operation source information, recorded only while source
/// tracking is enabled.
struct Track {
    location: &'static Location<'static>,
    #[cfg(feature = "operation-backtrace")]
    backtrace: std::backtrace::Backtrace,
}

/// State shared between a port, its completions, and the drain path.
///
/// `live` is the registry of operations submitted through this port whose
/// completion packet has not yet been dequeued. It governs rundown (its length
/// is the outstanding count) and answers whether an [`OperationId`] still names
/// an operation a packet is still coming for, which is what keeps a retained
/// identity from cancelling an operation that merely recycled its address.
/// Registration ends at dequeue, not at reclamation: a held [`Completion`] owns
/// its operation's storage but is no longer awaiting anything, and counting it
/// would make rundown wait for a packet it had already received.
/// `tracked` is consulted only when source tracking is enabled.
struct PortState {
    live: OperationRegistry,
    tracked: Mutex<HashMap<usize, Track>>,
}

impl PortState {
    fn new() -> Self {
        Self {
            live: OperationRegistry::new(),
            tracked: Mutex::new(HashMap::new()),
        }
    }
}

/// Lock a mutex, recovering the guard even if a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// An owned I/O completion port.
pub struct CompletionPort {
    handle: OwnedHandle,
    state: Arc<PortState>,
}

impl CompletionPort {
    /// Create a new completion port.
    ///
    /// `concurrency` is the maximum number of threads the system lets run
    /// completions for this port concurrently; zero means one per processor.
    pub fn new(concurrency: u32) -> io::Result<Self> {
        // SAFETY: creating a fresh port with no associated file handle.
        let handle = unsafe {
            CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, concurrency)
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call returned a fresh, exclusively owned port handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        Ok(Self {
            handle,
            state: Arc::new(PortState::new()),
        })
    }

    /// Associate an overlapped endpoint with this port under `key`.
    ///
    /// Completions for operations issued on the endpoint are delivered to this
    /// port and tagged with `key`. The association is permanent for the life of
    /// the handle, so the returned endpoint borrows the port.
    pub fn associate(
        &self,
        endpoint: UnassociatedEndpoint,
        key: usize,
    ) -> io::Result<AssociatedEndpoint<'_>> {
        let handle = endpoint.into_handle();
        // SAFETY: associating a valid handle with a valid port; the concurrency
        // argument is ignored when an existing port is supplied.
        let result = unsafe { CreateIoCompletionPort(handle.as_raw_handle(), self.raw(), key, 0) };
        if result.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(AssociatedEndpoint {
            port: self,
            handle,
            key,
        })
    }

    /// Post a user-defined wakeup packet to this port.
    ///
    /// The packet carries `key` and `bytes_transferred` with a null `OVERLAPPED`,
    /// which keeps it distinguishable from operation completions; identify it by
    /// its `key`.
    pub fn post(&self, key: usize, bytes_transferred: u32) -> io::Result<()> {
        // SAFETY: the port handle is valid; a null overlapped marks a user packet.
        let ok = unsafe {
            PostQueuedCompletionStatus(self.raw(), bytes_transferred, key, std::ptr::null())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn post_raw(
        &self,
        key: usize,
        bytes_transferred: u32,
        overlapped: *mut OVERLAPPED,
    ) -> io::Result<()> {
        // SAFETY: tests use this to simulate an operation completion for a live
        // operation's OVERLAPPED pointer.
        let ok = unsafe {
            PostQueuedCompletionStatus(self.raw(), bytes_transferred, key, overlapped.cast_const())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Dequeue one completion packet, waiting up to `timeout_ms` milliseconds.
    ///
    /// Returns `Ok(None)` when the wait times out with no packet. A packet is
    /// returned even when its operation failed; the failure is reported through
    /// [`Completion::error`].
    pub fn get(&self, timeout_ms: u32) -> io::Result<Option<Completion>> {
        let mut bytes_transferred: u32 = 0;
        let mut key: usize = 0;
        let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
        // SAFETY: all out-parameters are valid for the duration of the call.
        let ok = unsafe {
            GetQueuedCompletionStatus(
                self.raw(),
                &mut bytes_transferred,
                &mut key,
                &mut overlapped,
                timeout_ms,
            )
        };
        if ok != 0 {
            return Ok(Some(Completion {
                key,
                bytes_transferred,
                overlapped,
                error: None,
                // Deregister as the packet leaves the queue, recovering the
                // identity in the same step. See `deregister_dequeued`.
                id: self.deregister_dequeued(overlapped),
                claimed: Cell::new(false),
            }));
        }

        let error = io::Error::last_os_error();
        if overlapped.is_null() {
            if error.raw_os_error() == Some(WAIT_TIMEOUT as i32) {
                return Ok(None);
            }
            return Err(error);
        }
        // A packet for a failed operation was dequeued.
        Ok(Some(Completion {
            key,
            bytes_transferred,
            overlapped,
            error: Some(error),
            id: self.deregister_dequeued(overlapped),
            claimed: Cell::new(false),
        }))
    }

    /// Deregister an operation whose packet has just been dequeued, returning the
    /// identity the registry recorded for it.
    ///
    /// Registration ends at *dequeue*, not at reclamation, because the registry
    /// answers "is a packet still coming for this operation?" -- which is what
    /// [`run_down`](Self::run_down) waits on and what
    /// [`cancel`](AssociatedEndpoint::cancel) must not act against. Once the
    /// packet is off the queue neither is true any longer, and no further packet
    /// will ever arrive for it.
    ///
    /// Keeping the entry until the [`Completion`] was dropped instead made a
    /// held completion indistinguishable from an undelivered packet, so dropping
    /// the port while one was held blocked forever in an unbounded `get` waiting
    /// for a packet that had already been delivered. The completion still owns the
    /// operation's storage and still frees it on drop; that ownership is simply
    /// no longer expressed through the registry.
    ///
    /// A null pointer (a user packet) and an address this port never registered
    /// both return `None`, since `remove` reports only what it held.
    fn deregister_dequeued(&self, overlapped: *mut OVERLAPPED) -> Option<OperationId> {
        let id = self.state.live.remove(overlapped);
        if id.is_some() && crate::source_tracking_enabled() {
            lock(&self.state.tracked).remove(&(overlapped as usize));
        }
        id
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }

    /// The registry of operations submitted through this port and not yet
    /// reclaimed, for backends that must validate an identity before cancelling.
    ///
    /// Only the socket backend reaches for this; the file backend cancels
    /// through `AssociatedEndpoint`, which already holds the port.
    #[cfg(feature = "socket")]
    pub(crate) fn live_operations(&self) -> &OperationRegistry {
        &self.state.live
    }

    /// The number of operations submitted through this port whose completion
    /// packet has not yet been dequeued.
    ///
    /// A packet that has been dequeued is *not* counted, even if the
    /// [`Completion`] is still held and its storage not yet released. The count
    /// measures what the port is still waiting to deliver, which is what
    /// [`run_down`](Self::run_down) blocks on.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.state.live.len()
    }

    /// Block until a completion packet has been dequeued for every outstanding
    /// operation.
    ///
    /// Every outstanding operation must already be cancelled or otherwise
    /// destined to complete -- which closing or cancelling the endpoints
    /// guarantees -- or this waits indefinitely. Each packet dequeued here is
    /// reclaimed immediately, since the [`Completion`] this creates is dropped
    /// within the loop.
    ///
    /// A [`Completion`] held elsewhere does not keep this waiting: its packet has
    /// already been delivered, so it is not outstanding. It still owns the
    /// operation's storage, and still frees it when dropped, which may be after
    /// this returns and after the port itself is gone.
    ///
    /// The port is shareable, so another thread may be consuming completions at
    /// the same time. Each wait here is therefore bounded (`RUN_DOWN_POLL_MS`)
    /// and the live count is rechecked after it: a concurrent consumer can
    /// dequeue the last packet -- and clear its registry entry -- in the window
    /// between this loop observing a nonzero count and beginning its own wait,
    /// and an unbounded wait would then block forever on a packet no longer
    /// coming. Removing a registry entry does not wake a `GetQueuedCompletionStatus`
    /// already in progress, so the recheck, not a wakeup, is what ends the wait.
    pub fn run_down(&self) -> io::Result<()> {
        while self.outstanding() > 0 {
            self.get(RUN_DOWN_POLL_MS)?;
        }
        Ok(())
    }

    /// Submit an owned operation on this port, running the shared outstanding-
    /// operation accounting around a caller-supplied native call.
    ///
    /// This is the accounting core shared by every endpoint kind: `issue`
    /// receives only the stable `OVERLAPPED` pointer and performs the single
    /// native overlapped call (the endpoint supplies its own handle or socket by
    /// capture). The counting, source tracking, and reclamation on the
    /// synchronous and failure paths are identical regardless of endpoint kind.
    ///
    /// # Safety
    ///
    /// `issue` must start exactly one overlapped operation using the provided
    /// `OVERLAPPED` pointer and no other storage, and must classify the outcome
    /// correctly: [`Issued::Pending`] only when a completion packet will be
    /// delivered to this port, [`Issued::Completed`] only when the operation is
    /// already complete and no packet will arrive, and `Err` only when the
    /// submission failed and no completion will arrive.
    ///
    /// `issue` must not unwind. The operation is registered before it runs and
    /// deregistered only on the paths below; a panic out of `issue` skips that
    /// and -- because a panic before starting the I/O is indistinguishable from
    /// one after -- rundown could then wait forever for a packet that will never
    /// arrive. A closure that might panic must catch it and return `Err`.
    #[track_caller]
    pub(crate) unsafe fn submit_with<P, F>(&self, operation: Operation<P>, issue: F) -> Submitted<P>
    where
        P: Send,
        F: FnOnce(*mut OVERLAPPED) -> io::Result<Issued>,
    {
        // Transfer the operation's storage out; the caller (kernel) owns it until
        // it is reclaimed. `into_overlapped` arms the type-erased reclaim thunk.
        let overlapped = operation.into_overlapped();
        let identity = overlapped as usize;
        // Stamp this submission with a fresh generation, so the identity names
        // this operation and not whatever later operation may reuse the address.
        let id = OperationId::mint(overlapped);

        // Register before issuing so a completion cannot race ahead of the count.
        let state = &self.state;
        state.live.insert(id);
        let tracking = crate::source_tracking_enabled();
        if tracking {
            lock(&state.tracked).insert(
                identity,
                Track {
                    location: Location::caller(),
                    #[cfg(feature = "operation-backtrace")]
                    backtrace: std::backtrace::Backtrace::capture(),
                },
            );
        }

        match issue(overlapped) {
            Ok(Issued::Pending) => Submitted::Pending(id),
            Ok(Issued::Completed { bytes_transferred }) => {
                // Synchronous completion with no packet to arrive (the
                // skip-on-success case): balance the count and reclaim inline.
                state.live.remove(overlapped);
                if tracking {
                    lock(&state.tracked).remove(&identity);
                }
                // SAFETY: the operation completed synchronously and no packet
                // will arrive, so the kernel is done with the storage; reclaim
                // the box we just leaked, exactly once.
                let mut operation = unsafe { Operation::<P>::from_overlapped(overlapped) };
                operation.set_state(OperationState::Completed);
                Submitted::Completed {
                    operation,
                    bytes_transferred,
                }
            }
            Err(error) => {
                state.live.remove(overlapped);
                if tracking {
                    lock(&state.tracked).remove(&identity);
                }
                // SAFETY: no completion will arrive, so reclaim the operation we
                // just leaked, exactly once.
                let mut operation = unsafe { Operation::<P>::from_overlapped(overlapped) };
                operation.set_state(OperationState::Idle);
                Submitted::Failed { operation, error }
            }
        }
    }

    fn report_outstanding_at_drop(&self, count: usize) {
        let tracked = lock(&self.state.tracked);
        let mut message = format!(
            "windows-overlapped-io-sys: CompletionPort dropped with {count} operation(s) still \
             outstanding; call run_down() before dropping to control when this blocks."
        );
        if tracked.is_empty() {
            message.push_str(
                " Enable source tracking (WINDOWS_OVERLAPPED_IO_SYS_TRACK=1, or \
                 set_source_tracking) to identify the submit sites.",
            );
        } else {
            message.push_str(" Sources:");
            for track in tracked.values() {
                message.push_str("\n  - ");
                message.push_str(&track.location.to_string());
                #[cfg(feature = "operation-backtrace")]
                {
                    message.push_str("\n    backtrace:\n");
                    message.push_str(&track.backtrace.to_string());
                }
            }
        }
        eprintln!("{message}");
    }
}

impl fmt::Debug for CompletionPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionPort")
            .field("outstanding", &self.outstanding())
            .finish_non_exhaustive()
    }
}

impl Drop for CompletionPort {
    fn drop(&mut self) {
        let count = self.outstanding();
        if count == 0 {
            return;
        }
        // A blocking Drop signals that run_down() was skipped; name the sources.
        self.report_outstanding_at_drop(count);
        // Block until the kernel is done with every operation's storage.
        let _ = self.run_down();
    }
}

/// An overlapped endpoint bound to exactly one [`CompletionPort`].
///
/// The endpoint owns its handle and borrows the port it is associated with, so
/// the port cannot be dropped while any endpoint still routes completions to it.
/// It is intentionally not `Clone`.
#[derive(Debug)]
pub struct AssociatedEndpoint<'port> {
    port: &'port CompletionPort,
    handle: OwnedHandle,
    key: usize,
}

impl<'port> AssociatedEndpoint<'port> {
    /// Borrow the underlying handle for issuing native operations.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// The completion key packets from this endpoint are tagged with.
    #[must_use]
    pub fn key(&self) -> usize {
        self.key
    }

    /// The completion port this endpoint is associated with.
    #[must_use]
    pub fn port(&self) -> &'port CompletionPort {
        self.port
    }

    /// Submit an owned operation on this endpoint.
    ///
    /// `issue` performs the single native overlapped call using the endpoint's
    /// handle and the operation's stable `OVERLAPPED` pointer. It classifies the
    /// outcome as an [`Issued`]: [`Issued::Pending`] when a completion packet
    /// will be delivered, or [`Issued::Completed`] when the call finished
    /// synchronously and no packet will arrive -- the state a handle in
    /// `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode reports on synchronous
    /// success. It returns `Err` for an immediate failure that yields no
    /// completion.
    ///
    /// On the pending path the operation's storage is transferred to the kernel
    /// and recovered later with [`Completion::claim`]. On the synchronous and
    /// failure paths the operation is returned intact through [`Submitted`] so
    /// its storage can be reused or inspected.
    ///
    /// # Panics
    ///
    /// Panics if this port already has a live operation registered at the new
    /// operation's storage address. That cannot happen through ordinary use --
    /// `operation` owns freshly boxed storage -- and indicates a defect in this
    /// crate's own bookkeeping rather than in the calling code. See
    /// [`OperationRegistry::insert`] for the invariant involved.
    ///
    /// # Safety
    ///
    /// `issue` must start exactly one overlapped operation using the provided
    /// `OVERLAPPED` pointer and no other storage, and must classify the outcome
    /// correctly: [`Issued::Pending`] only when a completion packet will be
    /// delivered to this endpoint's port, [`Issued::Completed`] only when the
    /// operation is already complete and no packet will arrive, and `Err` only
    /// when the submission failed and no completion will arrive.
    ///
    /// `issue` must not unwind: a panic out of it can leave an operation
    /// registered with no completion coming, which makes rundown wait forever. A
    /// closure that might panic must catch it and return `Err`.
    #[track_caller]
    pub unsafe fn submit<P, F>(&self, operation: Operation<P>, issue: F) -> Submitted<P>
    where
        P: Send,
        F: FnOnce(BorrowedHandle<'_>, *mut OVERLAPPED) -> io::Result<Issued>,
    {
        let handle = self.handle();
        // SAFETY: `issue`'s safety contract (restated on this method) is exactly
        // what the shared core requires; the endpoint only supplies its handle.
        unsafe {
            self.port
                .submit_with(operation, move |overlapped| issue(handle, overlapped))
        }
    }

    /// Request cancellation of a single outstanding operation.
    ///
    /// Cancellation is only a request: the operation still completes, typically
    /// with `ERROR_OPERATION_ABORTED`, and that completion remains the point at
    /// which its storage is reclaimed with [`Completion::claim`].
    ///
    /// The identity is checked against this port's live operations first. An
    /// identity whose operation has already completed is rejected with
    /// [`io::ErrorKind::NotFound`] and no native call is made, even if another
    /// operation has since been given the same storage address -- so retaining
    /// an identity too long can never cancel an unrelated operation.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::NotFound`] if `id` no longer names a live
    /// operation, or the error from `CancelIoEx` if the native request fails.
    pub fn cancel(&self, id: OperationId) -> io::Result<()> {
        // The liveness check and the native call happen under one registry
        // guard; splitting them would let the address be recycled in between.
        self.port.state.live.cancel_if_live(id, || {
            // SAFETY: cancelling by a valid handle and an OVERLAPPED identity
            // the registry has confirmed still names a live operation, and which
            // cannot be reclaimed and reissued while the guard is held.
            let ok = unsafe { CancelIoEx(self.raw_handle(), id.as_ptr()) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
    }

    /// Request cancellation of every outstanding operation on this endpoint.
    pub fn cancel_all(&self) -> io::Result<()> {
        // SAFETY: a null OVERLAPPED cancels all operations on the handle.
        let ok = unsafe { CancelIoEx(self.raw_handle(), std::ptr::null()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn raw_handle(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

/// How the native call in [`AssociatedEndpoint::submit`] accepted an operation.
///
/// `issue` returns this to tell the backend whether a completion packet will be
/// delivered, so the port's outstanding-operation accounting stays correct.
#[derive(Debug, Clone, Copy)]
pub enum Issued {
    /// A completion packet will be delivered to the port; the operation's
    /// storage stays with the kernel until [`Completion::claim`] recovers it.
    /// This covers both native success that queues a packet and
    /// `ERROR_IO_PENDING`.
    Pending,
    /// The operation finished synchronously and no completion packet will
    /// arrive -- the outcome a handle in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`
    /// mode reports on synchronous success. `bytes_transferred` is the count the
    /// native call reported; the operation's storage is reclaimed inline and
    /// returned through [`Submitted::Completed`].
    Completed {
        /// The number of bytes the synchronous call transferred.
        bytes_transferred: u32,
    },
}

/// The outcome of [`AssociatedEndpoint::submit`].
#[derive(Debug)]
pub enum Submitted<P> {
    /// A completion will arrive; the storage was transferred to the kernel and
    /// is recovered later with [`Completion::claim`]. The [`OperationId`]
    /// identifies the in-flight operation for cancellation and matching.
    Pending(OperationId),
    /// The operation completed synchronously with no completion packet (the
    /// skip-on-success case); the operation is returned already reclaimed, with
    /// the bytes the native call transferred.
    Completed {
        /// The operation whose synchronous completion was observed inline.
        operation: Operation<P>,
        /// The number of bytes the synchronous call transferred.
        bytes_transferred: u32,
    },
    /// Submission failed immediately with no completion; the operation is
    /// returned so its storage can be reused or dropped.
    Failed {
        /// The operation whose submission failed.
        operation: Operation<P>,
        /// The immediate failure reported by the native call.
        error: io::Error,
    },
}

/// A completion packet dequeued from a [`CompletionPort`].
///
/// Dequeuing removes the operation from the port's outstanding set, so holding a
/// completion never blocks [`CompletionPort::run_down`] or the port's `Drop`. The
/// completion still owns the operation's storage until it is dropped or
/// [`claim`](Self::claim)ed, and may outlive the port it came from.
pub struct Completion {
    key: usize,
    bytes_transferred: u32,
    overlapped: *mut OVERLAPPED,
    error: Option<io::Error>,
    /// The identity of the operation this packet completes, recovered from the
    /// registry at dequeue time. `None` for a user packet, which has no
    /// operation. Stored whole rather than as a bare generation, so nothing here
    /// ever re-pairs an address with a generation.
    id: Option<OperationId>,
    claimed: Cell<bool>,
}

impl fmt::Debug for Completion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completion")
            .field("key", &self.key)
            .field("bytes_transferred", &self.bytes_transferred)
            .field("overlapped", &self.overlapped)
            .field("id", &self.id)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        // Claimed completions handed ownership to the caller; user packets carry
        // a null overlapped and own nothing.
        if self.claimed.get() || self.overlapped.is_null() {
            return;
        }
        // The registry entry is already gone -- dequeue removed it -- so this
        // only has to release the storage the completion still owns.
        // SAFETY: the completion arrived, so the kernel is done with the storage;
        // the operation's armed reclaim thunk frees the box exactly once.
        unsafe { crate::operation::reclaim_from_overlapped(self.overlapped) };
    }
}

impl Completion {
    /// The completion key the packet was tagged with.
    #[must_use]
    pub fn key(&self) -> usize {
        self.key
    }

    /// The number of bytes transferred by the operation.
    #[must_use]
    pub fn bytes_transferred(&self) -> u32 {
        self.bytes_transferred
    }

    /// The `OVERLAPPED` pointer identifying the completed operation.
    ///
    /// For a user packet this is whatever value was passed to
    /// [`CompletionPort::post`].
    #[must_use]
    pub fn overlapped_ptr(&self) -> *mut OVERLAPPED {
        self.overlapped
    }

    /// The identity of the operation this packet completes.
    ///
    /// It matches the [`OperationId`] that [`AssociatedEndpoint::submit`]
    /// returned for the operation, so a caller holding submission-time
    /// identities can match a completion against them directly. Returns `None`
    /// for a user packet from [`CompletionPort::post`], which completes no
    /// operation.
    #[must_use]
    pub fn id(&self) -> Option<OperationId> {
        self.id
    }

    /// The failure of the completed operation, if it did not succeed.
    #[must_use]
    pub fn error(&self) -> Option<&io::Error> {
        self.error.as_ref()
    }

    /// Recover the owned operation whose completion this is.
    ///
    /// # Safety
    ///
    /// This completion must have been produced by submitting an `Operation<P>`
    /// of this exact type through [`AssociatedEndpoint::submit`], and it must be
    /// claimed exactly once.
    pub unsafe fn claim<P>(&self) -> Operation<P> {
        // Mark claimed so this completion's own drop will not also reclaim it.
        // The registry entry was already removed at dequeue.
        self.claimed.set(true);
        // SAFETY: by this function's contract, the identity is a matching leaked
        // Operation<P>, reclaimed exactly once here.
        let mut operation = unsafe { Operation::<P>::from_overlapped(self.overlapped) };
        operation.set_state(OperationState::Completed);
        operation
    }
}

#[cfg(test)]
mod tests;
