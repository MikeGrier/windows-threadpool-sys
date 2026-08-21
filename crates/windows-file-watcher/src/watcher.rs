// Copyright (c) 2026 Mike Grier
//! The detailed single-directory watcher: arm a `ReadDirectoryChangesW`, decode
//! its completion, and re-arm.
//!
//! Reads are issued through `windows-threadpool-sys`' [`ThreadpoolIo`], which
//! owns the balanced `StartThreadpoolIo` accounting, and through the
//! `windows-overlapped-io-sys` submission seam, whose generation-stamped
//! `OperationId` is what stops a stale completion being misattributed to the
//! next read that reuses the same `OVERLAPPED` address (D-3/D-4).
//!
//! # Re-arm before processing
//!
//! The kernel stops recording changes for a directory between the moment a read
//! completes and the moment the next one is armed. That window is inherent to
//! `ReadDirectoryChangesW` -- it cannot be closed, only made small -- so the
//! completion path re-arms *first* and decodes afterwards. Decoding is pure
//! computation over a buffer the next read does not touch, so nothing about it
//! needs to happen before the re-arm. This crate surfaces the residual loss
//! honestly as a `Desync` rather than pretending it does not exist.
//!
//! # Why the buffer is `u32`-backed
//!
//! `ReadDirectoryChangesW` requires a DWORD-aligned buffer. A `Box<[u8]>` is only
//! byte-aligned, so the completion buffer is allocated as `Box<[u32]>` and viewed
//! as bytes. The allocation also has to be *stable*: the operation's payload
//! moves when the operation is boxed for submission, so the buffer cannot be
//! inline in the payload -- the `Box` indirection is what keeps the address the
//! kernel was given valid.

// The watcher is reached from the crate's public surface only once the M2.3
// delivery endpoint and the M3 monitor exist; until then only the unit tests
// construct it. Remove this once M2.3 wires it up.
#![allow(dead_code)]

use std::io;
use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, Mutex, OnceLock};

use windows_overlapped_io_sys::{Issued, Operation, Submitted, UnassociatedEndpoint};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SECURITY,
    FILE_NOTIFY_CHANGE_SIZE, ReadDirectoryChangesW,
};
use windows_threadpool_sys::io::{IoCompletion, ThreadpoolIo};

use crate::directory::DirectoryHandle;
use crate::notify::{DecodedBatch, DesyncCause, decode_batch};
use crate::queue::{Notification, Sender, WatchId};

/// The completion-buffer size used unless a caller chooses otherwise.
///
/// Large enough that ordinary activity does not overflow the kernel's per
/// directory record queue, small enough to be unremarkable per watched
/// directory. Overflow remains possible under a burst and is reported rather
/// than hidden (D-12).
pub(crate) const DEFAULT_BUFFER_BYTES: usize = 64 * 1024;

/// The bytes in one `u32`, the unit the completion buffer is allocated in so it
/// satisfies `ReadDirectoryChangesW`'s DWORD alignment requirement.
const BYTES_PER_WORD: usize = std::mem::size_of::<u32>();

/// Every `FILE_NOTIFY_CHANGE_*` class this crate can report.
///
/// M4 narrows this to the union of a directory's subscriptions; until then a
/// single watcher asks for everything so the decoder sees the full range of
/// actions.
pub(crate) const ALL_NOTIFY_FILTERS: u32 = FILE_NOTIFY_CHANGE_FILE_NAME
    | FILE_NOTIFY_CHANGE_DIR_NAME
    | FILE_NOTIFY_CHANGE_ATTRIBUTES
    | FILE_NOTIFY_CHANGE_SIZE
    | FILE_NOTIFY_CHANGE_LAST_WRITE
    | FILE_NOTIFY_CHANGE_CREATION
    | FILE_NOTIFY_CHANGE_SECURITY;

/// A DWORD-aligned completion buffer with a stable address.
///
/// See the module docs for why both properties are required.
struct ReadBuffer {
    words: Box<[u32]>,
}

impl ReadBuffer {
    /// Allocate a zeroed buffer of at least `bytes` bytes, rounded up to a whole
    /// number of DWORDs.
    fn new(bytes: usize) -> Self {
        let words = bytes.div_ceil(BYTES_PER_WORD).max(1);
        Self {
            words: vec![0_u32; words].into_boxed_slice(),
        }
    }

    /// The base address the kernel writes to. Stable across moves of the owner,
    /// because the data lives in the `Box`'s allocation rather than inline.
    fn as_mut_ptr(&mut self) -> *mut core::ffi::c_void {
        self.words.as_mut_ptr().cast()
    }

    /// The buffer length in bytes, as the Win32 parameter type.
    fn byte_len(&self) -> u32 {
        let bytes = self.words.len() * BYTES_PER_WORD;
        u32::try_from(bytes).expect("the completion buffer is sized in this crate and fits a u32")
    }

    /// The first `len` bytes, as the kernel described them.
    ///
    /// A `len` beyond the buffer is clamped rather than trusted; the decoder
    /// then reports the truncation as a desync.
    fn filled(&self, len: usize) -> &[u8] {
        let bytes = self.words.len() * BYTES_PER_WORD;
        let len = len.min(bytes);
        // SAFETY: `u32` has no padding or invalid bit patterns, so its backing
        // allocation is always a valid `[u8]` of four times the length, and `len`
        // has just been clamped to it.
        unsafe { std::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), len) }
    }
}

/// Where a watcher puts what it decodes.
///
/// A crate-owned queue sender, never a client callback: enqueueing is something
/// this crate does to storage it owns, so no client behaviour can reach the
/// cadence path (D-2/D-11).
type BatchSink = Sender;

/// How a watcher stopped, if it did.
///
/// A watcher that cannot re-arm has no way to keep reporting changes. Recording
/// why lets M5's fault machine classify it; for now it is observable state.
#[derive(Debug)]
pub(crate) struct ArmFailure {
    pub(crate) error: io::Error,
}

/// The shared state a completion callback reaches.
struct WatcherInner {
    /// Set once, immediately after construction. The callback needs the object
    /// that owns it, which cannot be supplied at construction time.
    io: OnceLock<ThreadpoolIo>,
    filter: u32,
    subtree: bool,
    buffer_bytes: usize,
    sink: BatchSink,
    /// The subscription every notification from this watcher is tagged with.
    watch: WatchId,
    /// Whether arming is still permitted, held under a lock rather than an
    /// atomic *deliberately*. Teardown must be able to establish that no further
    /// read can be submitted, and a flag checked before a submission leaves a
    /// window: the callback could pass the check, then teardown could cancel and
    /// begin waiting, and only then would the callback submit -- leaving a fresh
    /// pending read that rundown waits on forever, because nothing will complete
    /// it. Holding this lock across the submission means teardown's own
    /// acquisition waits for any in-flight submission to finish, after which no
    /// new one can start.
    may_arm: Mutex<bool>,
    /// The failure that stopped re-arming, if any.
    stopped: Mutex<Option<ArmFailure>>,
}

impl WatcherInner {
    /// Arm one read. Returns the submission outcome.
    ///
    /// The buffer travels as the operation's payload, so it lives exactly as
    /// long as the operation the kernel owns.
    fn arm(self: &Arc<Self>) -> Result<(), io::Error> {
        // Held for the whole submission; see the field's documentation.
        let may_arm = self
            .may_arm
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !*may_arm {
            return Ok(());
        }

        let Some(io) = self.io.get() else {
            // Unreachable in practice: `io` is set before the first arm and the
            // callback cannot run before a submission exists.
            return Err(io::Error::other("the watcher's I/O object is not yet set"));
        };

        let mut operation = Operation::new(ReadBuffer::new(self.buffer_bytes));
        // Read the buffer's address and length *before* submitting, because
        // `submit` consumes the operation. Both stay valid: the bytes live in the
        // payload's `Box`, whose allocation does not move when the operation is
        // boxed for submission.
        let buffer_ptr = operation.payload_mut().as_mut_ptr();
        let buffer_len = operation.payload().byte_len();
        let filter = self.filter;
        let subtree = i32::from(self.subtree);

        // SAFETY: issues exactly one overlapped `ReadDirectoryChangesW` against
        // the supplied `OVERLAPPED`, writing only into the operation's own
        // payload buffer, which the kernel owns until the completion arrives.
        // `lpBytesReturned` is null, which is what the SDK requires for an
        // asynchronous call, and no completion routine is used because the pool
        // delivers the completion. The closure cannot unwind: it performs one FFI
        // call and returns a classification.
        let submitted = unsafe {
            io.submit(operation, |handle, overlapped| {
                let ok = ReadDirectoryChangesW(
                    handle.as_raw_handle(),
                    buffer_ptr,
                    buffer_len,
                    subtree,
                    filter,
                    std::ptr::null_mut(),
                    overlapped,
                    None,
                );
                if ok != 0 {
                    // A synchronous success still delivers a completion, because
                    // the handle is not in skip-on-success mode.
                    return Ok(Issued::Pending);
                }
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                    return Ok(Issued::Pending);
                }
                Err(error)
            })
        };

        match submitted {
            Submitted::Pending(_) => Ok(()),
            // No handle this crate opens is in skip-on-success mode, so this arm
            // is not reachable; treat it as armed rather than inventing a
            // completion the pool will not deliver.
            Submitted::Completed { .. } => Ok(()),
            Submitted::Failed { error, .. } => Err(error),
        }
    }

    /// Handle one completion: re-arm, then decode and publish.
    fn on_completion(self: &Arc<Self>, completion: &IoCompletion) {
        // SAFETY: this object only ever submits `Operation<ReadBuffer>` (in
        // `arm` above), and each completion is claimed exactly once, here.
        let operation = unsafe { completion.claim::<ReadBuffer>() };

        if let Some(error) = completion.error() {
            // A cancelled read is teardown, not a fault: stay silent and do not
            // re-arm, or rundown would never converge.
            if error.raw_os_error() == Some(OPERATION_ABORTED) {
                return;
            }
            self.record_stop(error);
            return;
        }

        // Re-arm before decoding: the kernel records nothing for this directory
        // until the next read is outstanding, so that window is minimised by
        // doing the re-arm first. Decoding touches only this completed buffer.
        if let Err(error) = self.arm() {
            self.record_stop(error);
        }

        let transferred = completion.bytes_transferred();
        let batch = decode_batch(operation.payload().filled(transferred));
        self.publish(batch);
    }

    /// Tag a decoded batch with this watcher's subscription and enqueue it.
    ///
    /// An empty change list is dropped rather than enqueued: the kernel can
    /// complete a read carrying no records, and forwarding that would make a
    /// client's "I was woken, so something changed" reasoning false.
    fn publish(&self, batch: DecodedBatch) {
        let notification = match batch {
            DecodedBatch::Changes(changes) if changes.is_empty() => return,
            DecodedBatch::Changes(changes) => Notification::Batch {
                watch: self.watch,
                changes,
            },
            DecodedBatch::Desync(cause) => Notification::Desync {
                watch: self.watch,
                cause,
            },
        };
        self.sink.send(notification);
    }

    /// Record the failure that stopped this watcher, and tell the client the
    /// change stream has a hole in it.
    fn record_stop(&self, error: io::Error) {
        let mut stopped = self
            .stopped
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if stopped.is_none() {
            *stopped = Some(ArmFailure { error });
            drop(stopped);
            // Dropping out of the watch loop means changes from here on are
            // unobserved, which is precisely a desync (D-12). M5 replaces this
            // with re-establishment.
            self.publish(DecodedBatch::Desync(DesyncCause::Overflow));
        }
    }
}

/// `ERROR_OPERATION_ABORTED`, the completion status a cancelled read reports.
const OPERATION_ABORTED: i32 = windows_sys::Win32::Foundation::ERROR_OPERATION_ABORTED as i32;

/// A detailed watcher over one directory.
///
/// Owns the directory handle (through the thread pool's I/O object) and keeps a
/// read outstanding, re-arming after each completion.
pub(crate) struct DirectoryWatcher {
    inner: Arc<WatcherInner>,
}

impl DirectoryWatcher {
    /// Open `directory` and arm the first read.
    ///
    /// Everything decoded is tagged with `watch` and enqueued on `sink`.
    ///
    /// # Errors
    ///
    /// Returns the error from binding the handle to the pool or from the first
    /// `ReadDirectoryChangesW`.
    pub(crate) fn start(
        directory: DirectoryHandle,
        subtree: bool,
        watch: WatchId,
        sink: Sender,
    ) -> io::Result<Self> {
        Self::start_with(directory, subtree, DEFAULT_BUFFER_BYTES, watch, sink)
    }

    /// As [`start`](Self::start), with an explicit completion-buffer size.
    ///
    /// A small buffer is how a test forces the kernel's overflow signal.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start).
    pub(crate) fn start_with(
        directory: DirectoryHandle,
        subtree: bool,
        buffer_bytes: usize,
        watch: WatchId,
        sink: Sender,
    ) -> io::Result<Self> {
        let inner = Arc::new(WatcherInner {
            io: OnceLock::new(),
            filter: ALL_NOTIFY_FILTERS,
            subtree,
            buffer_bytes,
            sink,
            watch,
            may_arm: Mutex::new(true),
            stopped: Mutex::new(None),
        });

        // The callback holds a `Weak`, never a strong reference. A strong one
        // would be a cycle -- inner owns the I/O object, which owns the callback
        // -- so the watcher could never drop. It also gives re-arm suppression
        // for free: once the owner drops, the upgrade fails and the callback
        // stops re-arming, which is exactly what lets rundown converge.
        let weak = Arc::downgrade(&inner);

        // SAFETY: `DirectoryHandle` only ever opens with FILE_FLAG_OVERLAPPED
        // (see its module), which is the association this constructor requires,
        // and ownership of the handle transfers here exclusively.
        let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(directory.into_handle()) };

        let io = ThreadpoolIo::new(
            endpoint,
            move |completion: &IoCompletion| {
                let Some(inner) = weak.upgrade() else {
                    // The watcher is being torn down. Claim the storage so it is
                    // not leaked, then do nothing else -- in particular, do not
                    // re-arm.
                    // SAFETY: this object only ever carries
                    // `Operation<ReadBuffer>`, claimed exactly once here.
                    drop(unsafe { completion.claim::<ReadBuffer>() });
                    return;
                };
                inner.on_completion(completion);
            },
            None,
        )?;

        inner
            .io
            .set(io)
            .unwrap_or_else(|_| unreachable!("the I/O object is set exactly once, here"));

        inner.arm()?;
        Ok(Self { inner })
    }

    /// The failure that stopped this watcher re-arming, if any.
    pub(crate) fn stop_reason(&self) -> Option<io::Error> {
        self.inner
            .stopped
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .map(|failure| {
                failure.error.raw_os_error().map_or_else(
                    || io::Error::other("watcher stopped"),
                    io::Error::from_raw_os_error,
                )
            })
    }

    /// Whether the watcher is still re-arming.
    pub(crate) fn is_watching(&self) -> bool {
        self.inner
            .stopped
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_none()
    }
}

impl std::fmt::Debug for DirectoryWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryWatcher")
            .field("watching", &self.is_watching())
            .finish_non_exhaustive()
    }
}

impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        // Close the door on re-arming *before* cancelling. Doing it the other way
        // round would let a completion callback that is already running submit a
        // fresh read after the cancellation, and rundown would then wait forever
        // for a completion that only a future directory change could produce.
        {
            let mut may_arm = self
                .inner
                .may_arm
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *may_arm = false;
        }

        // Now nothing new can be submitted, so cancelling retires everything
        // that is outstanding and rundown converges. M2.4 formalises this.
        if let Some(io) = self.inner.io.get() {
            let _ = io.cancel_all();
            io.run_down();
        }
    }
}

#[cfg(test)]
mod tests;
