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

use std::collections::HashMap;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use windows_overlapped_io_sys::{Issued, Operation, Submitted, UnassociatedEndpoint};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SECURITY,
    FILE_NOTIFY_CHANGE_SIZE, ReadDirectoryChangesW,
};
use windows_threadpool_sys::io::{IoCompletion, ThreadpoolIo};
use windows_threadpool_sys::work::ThreadpoolWork;

use crate::directory::DirectoryHandle;
use crate::notify::{DecodedBatch, DesyncCause, decode_batch};
use crate::queue::{Notification, Resume, Sender, WatchId};
use crate::route::{Route, RouteScope};

/// The completion-buffer size used unless a caller chooses otherwise.
///
/// Large enough that ordinary activity does not overflow the kernel's per
/// directory record queue, small enough to be unremarkable per watched
/// directory. Overflow remains possible under a burst and is reported rather
/// than hidden (D-12).
pub const DEFAULT_BUFFER_BYTES: usize = 64 * 1024;

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

/// Whether the watcher may still submit a read, and if not, why.
///
/// A bare boolean would be enough for teardown alone, but "not re-arming" is a
/// state three different decisions reach for different reasons -- teardown here,
/// a latched fault (D-28), and queue backpressure (D-29) -- and only the first is
/// permanent. Naming the reason now means the later two add a variant rather than
/// discovering that a `bool` cannot say which of them stopped the watcher, and it
/// keeps `stop_reason` from having to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmGate {
    /// Reads may be submitted.
    Open,
    /// The client's queue is full, so re-arming would produce a batch with
    /// nowhere to go. Transient: the loss becomes a grace period in the kernel's
    /// own change buffer, and the watch resumes when the client drains (D-29).
    Backpressured,
    /// Transiently closed while the directory is being reopened to widen its
    /// reach to recursive (M4.4/D-6). Self-resolving: it becomes `Open` once the
    /// new endpoint is armed, or `TornDown` if teardown wins the race.
    Reopening,
    /// Torn down. Permanent: a watcher never re-opens.
    TornDown,
}

/// The shared state a completion callback reaches.
struct WatcherInner {
    /// The endpoint currently in use. Replaced, not merely set once: widening a
    /// watcher's reach to recursive (M4.4) reopens the directory rather than
    /// cancelling and resubmitting on the same handle, because the latter was
    /// measured to leave the filesystem's recursive attachment unchanged --
    /// direct children kept being reported, nested ones never were. See
    /// [`WatcherInner::reopen`].
    io: Mutex<Option<ThreadpoolIo>>,
    /// Set once, as for `io` before it started being replaced. Queues the re-arm
    /// that ends a backpressure pause onto this crate's own pool, so it never
    /// runs on a client's thread.
    resume_work: OnceLock<ThreadpoolWork>,
    /// Every `FILE_NOTIFY_CHANGE_*` class this watcher asks for. Constant rather
    /// than a per-subscription union: no subscription can select a filter yet
    /// (`WatchOptions` has no such field), so the union over any set of
    /// subscriptions is trivially this same constant.
    filter: u32,
    buffer_bytes: usize,
    /// Every subscription this directory currently serves, keyed by the
    /// identifier that tags its notifications (D-6). Read at every arm to decide
    /// the kernel's own `bWatchSubtree` reach, and at every completion to
    /// de-multiplex the decoded batch (D-6/D-7).
    routes: Mutex<HashMap<WatchId, Route>>,
    /// Whether arming is still permitted, held under a lock rather than an
    /// atomic *deliberately*. Teardown must be able to establish that no further
    /// read can be submitted, and a flag checked before a submission leaves a
    /// window: the callback could pass the check, then teardown could cancel and
    /// begin waiting, and only then would the callback submit -- leaving a fresh
    /// pending read that rundown waits on forever, because nothing will complete
    /// it. Holding this lock across the submission means teardown's own
    /// acquisition waits for any in-flight submission to finish, after which no
    /// new one can start.
    gate: Mutex<ArmGate>,
    /// The failure that stopped re-arming, if any.
    stopped: Mutex<Option<io::Error>>,
}

impl WatcherInner {
    /// Arm one read. Returns the submission outcome.
    ///
    /// The buffer travels as the operation's payload, so it lives exactly as
    /// long as the operation the kernel owns.
    fn arm(self: &Arc<Self>) -> Result<(), io::Error> {
        // Held for the whole submission; see the field's documentation.
        let mut gate = self
            .gate
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if *gate == ArmGate::TornDown {
            return Ok(());
        }
        self.arm_locked(&mut gate)
    }

    /// Re-arm if backpressure was the only thing stopping this watcher.
    ///
    /// Only from [`ArmGate::Backpressured`], never from [`ArmGate::Open`]: an
    /// open gate means a read is already outstanding, and a second one against
    /// the same handle would be a duplicate the design never issues.
    fn resume_arm(self: &Arc<Self>) {
        let mut gate = self
            .gate
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if *gate != ArmGate::Backpressured {
            return;
        }
        if let Err(error) = self.arm_locked(&mut gate) {
            drop(gate);
            self.record_stop(error);
        }
    }

    /// Submit a read, or record why not. The gate lock is held throughout (D-23).
    fn arm_locked(self: &Arc<Self>, gate: &mut ArmGate) -> Result<(), io::Error> {
        // Checked here rather than at the enqueue: refusing to arm leaves the
        // changes in the kernel's own buffer, which is a grace period rather than
        // a loss, where discovering a full queue after the read has completed is
        // a batch with nowhere to go (D-29). Paused only when *every* route is
        // full: a route with room still benefits from the grace period, and one
        // with none falls back to its own latch (D-29's fallback) -- arming
        // would not have helped it either way.
        if !self.any_route_has_room() {
            *gate = ArmGate::Backpressured;
            // Re-checked *under this lock* because a drain may have freed a slot
            // since the check above. Without it the wake could be missed: a
            // resume that ran before the gate was set would find it open and do
            // nothing, and the watcher would stay parked with room available and
            // nothing left to prod it.
            if !self.any_route_has_room() {
                return Ok(());
            }
        }
        *gate = ArmGate::Open;

        let io_guard = lock(&self.io);
        let Some(io) = io_guard.as_ref() else {
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
        // Read fresh at every arm: a route joining or leaving between arms
        // changes what the *next* read needs (D-6). Downstream routing narrows
        // each route back to what it actually asked for regardless of what the
        // kernel was told to reach.
        let subtree = i32::from({
            let routes = lock(&self.routes);
            routes
                .values()
                .any(|route| route.scope.needs_kernel_subtree())
        });

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
            // re-arm, or rundown would never converge. Widening to recursive
            // reach (M4.4) no longer cancels the live read to do it -- it
            // reopens the directory instead (see `reopen`), so every abort seen
            // here is teardown's.
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
    ///
    /// De-multiplexes across every current route (D-6): a batch is filtered to
    /// each route's own scope, and a route whose filtered subset is empty gets
    /// nothing, for the same reason an empty completion gets nothing. A desync
    /// is never filtered -- it means "you may have missed something in this
    /// directory", which is equally true for every subscription within it.
    fn publish(&self, batch: DecodedBatch) {
        let routes = lock(&self.routes);
        match batch {
            DecodedBatch::Changes(changes) if changes.is_empty() => {}
            DecodedBatch::Changes(changes) => {
                for route in routes.values() {
                    let matched = route.select(&changes);
                    if matched.is_empty() {
                        continue;
                    }
                    // Observation reserves nothing, so this may be latched rather
                    // than queued (D-33); M3.7 responds by not re-arming while
                    // every route is full, turning the loss into a grace period.
                    let _ = route.sink.send(Notification::Batch {
                        watch: route.watch,
                        changes: matched,
                    });
                }
            }
            DecodedBatch::Desync(cause) => {
                for route in routes.values() {
                    let _ = route.sink.send(Notification::Desync {
                        watch: route.watch,
                        cause,
                    });
                }
            }
        }
    }

    /// Whether at least one current route's sink has room for a best-effort
    /// send.
    fn any_route_has_room(&self) -> bool {
        lock(&self.routes)
            .values()
            .any(|route| route.sink.has_room())
    }

    /// Cancel the outstanding read solely to pick up a newly widened reach.
    ///
    /// Reopens the directory rather than cancelling and resubmitting on the
    /// same handle. That was tried first and measured not to work: a live
    /// `ReadDirectoryChangesW` handle's recursive attachment does not appear to
    /// take effect (or take effect reliably) from a later call that merely
    /// changes `bWatchSubtree` on the same handle -- a direct child kept being
    /// reported after the widen, but nothing nested inside it ever was. A fresh
    /// `CreateFileW` does not have this problem.
    ///
    /// A no-op when there is nothing live to widen: [`ArmGate::TornDown`] has
    /// nothing left to reopen, and this is only ever called while the gate is
    /// [`ArmGate::Open`] (see [`DirectoryWatcher::add_route`]).
    ///
    /// # Errors
    ///
    /// Returns the error from binding the reopened handle to the pool or from
    /// arming its first read.
    fn reopen(self: &Arc<Self>, handle: DirectoryHandle) -> io::Result<()> {
        {
            let mut gate = lock(&self.gate);
            if *gate == ArmGate::TornDown {
                return Ok(());
            }
            // Transient and self-resolving: cleared to `Open` below once the new
            // endpoint is armed, or left as `TornDown` if teardown wins the race
            // with this reopen.
            *gate = ArmGate::Reopening;
        }

        // Tear down the old endpoint fully -- cancelled, then no operation
        // outstanding and no callback executing -- before anything touches the
        // new one, the same ordering `stop()` relies on (D-23). Not held across
        // this: `run_down` can block, and the gate lock is not this crate's to
        // hold across a wait.
        if let Some(old) = lock(&self.io).take() {
            let _ = old.cancel_all();
            old.run_down();
        }

        // The callback closure has the same shape as `DirectoryWatcher::start`'s:
        // a `Weak` (never a strong reference, for the same cycle-avoidance
        // reason), claiming and discarding a completion once the owner is gone,
        // otherwise dispatching to `on_completion`.
        let weak = Arc::downgrade(self);
        // SAFETY: `handle` was opened the same way every `DirectoryHandle` is
        // (see its module), which is the association this endpoint requires, and
        // ownership transfers here exclusively.
        let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(handle.into_handle()) };
        let new_io = ThreadpoolIo::new(
            endpoint,
            move |completion: &IoCompletion| {
                let Some(inner) = weak.upgrade() else {
                    // SAFETY: this object only ever carries
                    // `Operation<ReadBuffer>`, claimed exactly once here.
                    drop(unsafe { completion.claim::<ReadBuffer>() });
                    return;
                };
                inner.on_completion(completion);
            },
            None,
        )?;
        *lock(&self.io) = Some(new_io);

        {
            let mut gate = lock(&self.gate);
            if *gate == ArmGate::TornDown {
                // Torn down while this was in flight: the fresh endpoint was
                // just installed with nothing outstanding on it, so `stop()`'s
                // own cancel/run_down (already run once, above, against the old
                // one) has nothing further to do here; leave it be rather than
                // arming a read teardown never asked for.
                return Ok(());
            }
            *gate = ArmGate::Open;
        }
        self.arm()
    }

    /// Record the failure that stopped this watcher, and tell the client the
    /// change stream has a hole in it.
    fn record_stop(&self, error: io::Error) {
        let mut stopped = lock(&self.stopped);
        if stopped.is_none() {
            *stopped = Some(error);
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

impl Resume for WatcherInner {
    /// Queue the re-arm rather than performing it here.
    ///
    /// This runs on whichever thread drained the client's queue, which may be the
    /// client's own. Arming takes the gate lock and issues a read, and teardown
    /// waits on that same lock -- so doing it here would put our critical section
    /// on a thread we do not control. Submitting work is a single non-blocking
    /// call that touches nothing of ours, and the re-arm then happens on this
    /// crate's own pool.
    fn resume(&self) {
        if let Some(work) = self.resume_work.get() {
            work.submit();
        }
    }
}

/// A detailed watcher over one directory.
///
/// Owns the directory handle (through the thread pool's I/O object) and keeps a
/// read outstanding, re-arming after each completion.
pub struct DirectoryWatcher {
    inner: Arc<WatcherInner>,
}

impl DirectoryWatcher {
    /// Open `directory` and arm the first read, with one initial route.
    ///
    /// # Errors
    ///
    /// Returns the error from binding the handle to the pool or from the first
    /// `ReadDirectoryChangesW`.
    pub(crate) fn start(
        directory: DirectoryHandle,
        watch: WatchId,
        scope: RouteScope,
        sink: Sender,
    ) -> io::Result<Self> {
        Self::start_with(directory, DEFAULT_BUFFER_BYTES, watch, scope, sink)
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
        buffer_bytes: usize,
        watch: WatchId,
        scope: RouteScope,
        sink: Sender,
    ) -> io::Result<Self> {
        let mut routes = HashMap::new();
        let initial_sink = sink.clone();
        routes.insert(watch, Route { watch, scope, sink });

        let inner = Arc::new(WatcherInner {
            io: Mutex::new(None),
            resume_work: OnceLock::new(),
            filter: ALL_NOTIFY_FILTERS,
            buffer_bytes,
            routes: Mutex::new(routes),
            gate: Mutex::new(ArmGate::Open),
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
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .replace(io);

        // As with the completion callback, a `Weak` rather than a strong
        // reference: the work object lives in `inner`, so a strong one would be a
        // cycle, and a failed upgrade during teardown is exactly the suppression
        // that lets rundown converge.
        let resuming = Arc::downgrade(&inner);
        let resume_work = ThreadpoolWork::new(
            move || {
                if let Some(inner) = resuming.upgrade() {
                    inner.resume_arm();
                }
            },
            None,
        )?;
        inner
            .resume_work
            .set(resume_work)
            .unwrap_or_else(|_| unreachable!("the resume work is set exactly once, here"));

        // Registered before the first arm, so a watcher that is backpressured
        // from the outset is still prodded when the client drains.
        initial_sink.register_resume(&inner);

        inner.arm()?;
        Ok(Self { inner })
    }

    /// Add a subscription to this directory (D-6), reopening to widen the
    /// kernel's own reach if this is the first route that needs recursion.
    ///
    /// `fresh_handle` is a handle already opened onto this same directory --
    /// the caller had to open one anyway to discover the directory's identity
    /// and find this watcher to coalesce onto (D-6) -- reused here rather than
    /// opening a second one, and simply dropped if it turns out not to be
    /// needed.
    ///
    /// The other direction -- a recursive route leaving -- never needs the
    /// mirror-image contraction: an over-broad read is filtered back down to
    /// what each remaining route asked for by [`Route::select`], so it costs
    /// nothing but a few extra bytes to decode, where a read that is too
    /// *narrow* would silently under-report.
    ///
    /// A reopen failure stops the whole watcher (D-15's rearm-and-retry
    /// classification), reported to every route it currently serves rather than
    /// only the one that triggered it, since they now share one endpoint.
    pub(crate) fn add_route(
        &self,
        watch: WatchId,
        scope: RouteScope,
        sink: Sender,
        fresh_handle: DirectoryHandle,
    ) {
        let widen = scope.needs_kernel_subtree();
        let already_recursive = {
            let mut routes = lock(&self.inner.routes);
            let already = routes
                .values()
                .any(|route| route.scope.needs_kernel_subtree());
            routes.insert(
                watch,
                Route {
                    watch,
                    scope,
                    sink: sink.clone(),
                },
            );
            already
        };
        // Registered even when this route does not widen anything: it may still
        // need the resume prod later, on its own account, if its sink saturates
        // while this watcher is paused for some other route.
        sink.register_resume(&self.inner);
        if widen
            && !already_recursive
            && let Err(error) = self.inner.reopen(fresh_handle)
        {
            self.inner.record_stop(error);
        }
    }

    /// Remove a subscription, returning how many routes remain.
    ///
    /// The caller tears this watcher down entirely once this reaches zero;
    /// removing the route itself never requires re-arming (see
    /// [`DirectoryWatcher::add_route`]).
    pub(crate) fn remove_route(&self, watch: WatchId) -> usize {
        let mut routes = lock(&self.inner.routes);
        routes.remove(&watch);
        routes.len()
    }

    /// The failure that stopped this watcher re-arming, if any.
    pub fn stop_reason(&self) -> Option<io::Error> {
        lock(&self.inner.stopped).as_ref().map(|error| {
            error.raw_os_error().map_or_else(
                || io::Error::other("watcher stopped"),
                io::Error::from_raw_os_error,
            )
        })
    }

    /// Whether the watcher is still re-arming.
    ///
    /// A backpressured watcher counts: it is paused, not stopped, and resumes on
    /// its own when the client drains (D-29). D-31 is why the distinction is
    /// observable at all -- a paused watcher is otherwise indistinguishable from
    /// a quiet directory.
    pub fn is_watching(&self) -> bool {
        lock(&self.inner.stopped).is_none() && self.gate() != ArmGate::TornDown
    }

    /// The current arm gate.
    pub fn gate(&self) -> ArmGate {
        *lock(&self.inner.gate)
    }

    /// Stop watching: refuse further reads, cancel the outstanding one, and wait
    /// for every completion callback to finish (D-20).
    ///
    /// Idempotent, so a caller may stop explicitly and still let `Drop` run.
    /// After it returns, no callback for this watcher is executing or can start,
    /// and nothing further is delivered.
    ///
    /// Must not be called from inside this watcher's own completion callback: it
    /// waits for that callback to finish, so it would wait on itself. Nothing in
    /// this crate does, because the only caller is teardown on an owning thread.
    pub fn stop(&self) {
        // Close the gate *before* cancelling. The other order leaves a window in
        // which a completion callback already running submits a fresh read after
        // the cancellation, and rundown then waits forever for a completion only
        // a future directory change could produce (D-23).
        {
            let mut gate = lock(&self.inner.gate);
            *gate = ArmGate::TornDown;
        }

        // With the gate closed, nothing new can be submitted, so cancelling
        // retires everything outstanding and rundown converges. Both calls are
        // safe to repeat: `cancel_all` reports `ERROR_NOT_FOUND` when nothing is
        // outstanding, and `run_down` returns immediately on an empty registry.
        if let Some(io) = lock(&self.inner.io).as_ref() {
            let _ = io.cancel_all();
            io.run_down();
        }
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
        // Teardown is the same operation whether it was asked for or implied, so
        // there is one implementation and `Drop` is only the implicit trigger.
        // It is idempotent, so an explicit `stop()` beforehand costs nothing.
        self.stop();

        // The queue sender lives in `inner`, which drops after this: with every
        // callback finished and no strong reference left, the sender is released
        // and the client's receiver observes the disconnect rather than blocking
        // forever on a queue nothing can fill again.
    }
}

/// Lock, recovering the guard if a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests;
