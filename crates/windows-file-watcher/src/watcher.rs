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

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use windows_overlapped_io_sys::{Issued, Operation, Submitted, UnassociatedEndpoint};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SECURITY,
    FILE_NOTIFY_CHANGE_SIZE, FindNextChangeNotification, ReadDirectoryChangesW,
};
use windows_threadpool_sys::io::{IoCompletion, ThreadpoolIo};
use windows_threadpool_sys::timer::ThreadpoolTimer;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitActivation};
use windows_threadpool_sys::work::ThreadpoolWork;

use crate::coarse::CoarseHandle;
use crate::directory::{DirectoryHandle, OpenFailure, classify};
use crate::notify::{DecodedBatch, DesyncCause, decode_batch};
use crate::queue::{Notification, Resume, WatchId};
use crate::retry::{FaultOperation, WatchMode, clamp};
use crate::route::Route;
use crate::watch::RetryMode;

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
/// Every watcher always asks for all of them. This is the permanent arrangement,
/// not a placeholder for a later per-subscription union: D-77 withdrew the
/// change-type filter outright, so there is nothing to narrow this to and never
/// will be (D-51).
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
    /// Transiently closed while the monitor works to re-establish this watcher
    /// after an open or arm fault (D-14/D-15/M5.1): asking any interactive
    /// routes, waiting for their answers, backed off on a timer, or in the
    /// middle of a retry attempt. Self-resolving: it becomes `Open` once a retry
    /// succeeds, or `TornDown` if teardown wins the race. A permanent open
    /// failure (D-22) is the one edge that does not return here -- see
    /// `WatcherInner::stopped`.
    Faulted,
    /// Torn down. Permanent: a watcher never re-opens.
    TornDown,
}

/// Fault-recovery state (D-14/D-15/D-27/M5.1): held while this watcher is
/// working to re-establish itself, and absent otherwise.
struct FaultState {
    /// Which operation faulted -- an open (of the reopened directory) or an
    /// arm (a live `ReadDirectoryChangesW` completion or resubmission). Each
    /// carries its own default delay (D-15).
    operation: FaultOperation,
    /// Interactive routes (D-27) that have not yet answered this fault's
    /// question. Emptied by an answer arriving, or by the route leaving
    /// (M5.5): either way, once empty the resolved delay is scheduled.
    awaiting: HashSet<WatchId>,
    /// The soonest delay any answer has named so far, seeded at the
    /// operation's default (D-27) and only ever lowered.
    earliest: Duration,
}

/// Which tier is servicing a directory's watch (D-17): the preferred detailed
/// path, or the coarse floor a detailed arm downgrades to (M6.3).
enum Endpoint {
    /// `ReadDirectoryChangesW` on a `ThreadpoolIo`.
    Detailed(ThreadpoolIo),
    /// `FindFirstChangeNotification` on a `ThreadpoolWait` (M6.1/M6.2).
    Coarse(ThreadpoolWait),
}

/// The shared state a completion callback reaches.
struct WatcherInner {
    /// The path this directory was opened from, kept so re-establishment
    /// (M5.1) knows what to reopen -- a live handle cannot be recovered once
    /// its directory is gone, but the path can still be retried. Also what a
    /// downgrade to coarse (M6.3) opens its own handle from, since a coarse
    /// watch is a wholly separate handle from the detailed one.
    path: PathBuf,
    /// The endpoint currently in use. Replaced, not merely set once: widening a
    /// watcher's reach to recursive (M4.4) reopens the directory rather than
    /// cancelling and resubmitting on the same handle, because the latter was
    /// measured to leave the filesystem's recursive attachment unchanged --
    /// direct children kept being reported, nested ones never were. See
    /// [`WatcherInner::reopen`]. Re-establishment (M5.1) reuses the same
    /// mechanism after a fresh open, and so does a tier downgrade (M6.3): both
    /// are "tear down whatever is here, install something new".
    endpoint: Mutex<Option<Endpoint>>,
    /// Set once, as for `endpoint` before it started being replaced. Queues the
    /// re-arm that ends a backpressure pause onto this crate's own pool, so it
    /// never runs on a client's thread.
    resume_work: OnceLock<ThreadpoolWork>,
    /// Set once at construction. Fires the next re-establish attempt after a
    /// fault's resolved delay (D-27); re-armed with a fresh due time, never
    /// recreated, across however many faults this watcher lives through.
    retry_timer: OnceLock<ThreadpoolTimer>,
    /// Every `FILE_NOTIFY_CHANGE_*` class this watcher asks for. Constant rather
    /// than a per-subscription union: no subscription can select a filter
    /// (`WatchOptions` has no such field, and D-77 withdrew the feature outright),
    /// so the union over any set of subscriptions is trivially this same
    /// constant. Shared by both tiers: the wire type (`FILE_NOTIFY_CHANGE`, a
    /// `u32`) is identical between `ReadDirectoryChangesW` and
    /// `FindFirstChangeNotificationW`.
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
    /// Held while this watcher is working to re-establish itself; absent while
    /// watching normally. See [`FaultState`].
    fault: Mutex<Option<FaultState>>,
    /// The failure that stopped re-arming *permanently*, if any. Only reached
    /// by a re-establish attempt that finds its target permanently unwatchable
    /// (D-22's `NotADirectory`/`InvalidPath`) -- every other failure retries
    /// indefinitely through `fault` instead (D-14).
    stopped: Mutex<Option<io::Error>>,
    /// M6.4's test seam: when set, `reopen` skips the detailed attempt entirely
    /// and establishes coarse from the start, regardless of what the
    /// underlying volume actually supports. Always present rather than
    /// `#[cfg(test)]`-gated (one bool costs nothing and never leaves the
    /// crate), but never set outside a test.
    force_coarse: AtomicBool,
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
            // An arm failure is always retry-class (D-15), never terminal.
            self.enter_fault(error, FaultOperation::Arm);
        }
    }

    /// Submit a read (or arm the coarse wait), or record why not. The gate
    /// lock is held throughout (D-23). Dispatches on which tier is currently
    /// installed (D-17); backpressure (D-29) applies uniformly to both, since
    /// a client's queue does not care which tier filled it.
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

        let endpoint_guard = lock(&self.endpoint);
        match endpoint_guard.as_ref() {
            Some(Endpoint::Detailed(io)) => self.arm_detailed_read(io),
            Some(Endpoint::Coarse(wait)) => {
                wait.arm(None);
                Ok(())
            }
            None => {
                // Unreachable in practice: an endpoint is installed before the
                // first arm and no callback can run before one exists.
                Err(io::Error::other("the watcher's endpoint is not yet set"))
            }
        }
    }

    /// Submit one overlapped `ReadDirectoryChangesW` against `io`.
    fn arm_detailed_read(&self, io: &ThreadpoolIo) -> Result<(), io::Error> {
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
            // Every other completion failure is an arm-class fault (D-15):
            // re-established indefinitely rather than treated as terminal
            // (D-14). `stopped` is reserved for the one edge that genuinely
            // cannot recover -- a permanent open failure discovered while
            // re-establishing.
            self.enter_fault(error, FaultOperation::Arm);
            return;
        }

        // Re-arm before decoding: the kernel records nothing for this directory
        // until the next read is outstanding, so that window is minimised by
        // doing the re-arm first. Decoding touches only this completed buffer.
        if let Err(error) = self.arm() {
            self.enter_fault(error, FaultOperation::Arm);
        }

        let transferred = completion.bytes_transferred();
        let batch = decode_batch(operation.payload().filled(transferred));
        self.publish(batch);
    }

    /// Handle one coarse activation: re-arm, then publish (M6.2).
    ///
    /// The handle stays signalled until `FindNextChangeNotification` is called,
    /// so that has to happen *before* re-arming the wait -- otherwise the pool
    /// would see the handle still signalled and could queue the next activation
    /// immediately, overlapping this one. A coarse activation carries no detail
    /// at all (unlike a detailed completion, which at least has a buffer to
    /// decode), so the whole report is `Desync { Coarse }` (D-17).
    fn on_activation(self: &Arc<Self>, activation: &WaitActivation<'_>) {
        // SAFETY: the endpoint owns this handle for as long as it is installed,
        // and this runs only from within the wait's own callback while it
        // still is.
        let reset = unsafe { FindNextChangeNotification(activation.handle().as_raw_handle()) };
        if reset == 0 {
            // A failed reset leaves the handle's signalled state undefined, so
            // re-arming on top of it could wedge (repeated callbacks) or wait
            // on a handle that will never signal again -- treat it exactly
            // like any other arm-class fault (D-15) rather than proceeding.
            self.enter_fault(io::Error::last_os_error(), FaultOperation::Arm);
            return;
        }
        if let Err(error) = self.arm() {
            self.enter_fault(error, FaultOperation::Arm);
            return;
        }
        self.publish(DecodedBatch::Desync(DesyncCause::Coarse));
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

    /// Begin (or restart) fault recovery for this watcher (D-14/D-15/M5.1).
    ///
    /// Closes the gate, tells every route the watch is suspended (opt-in,
    /// D-13), and asks every interactive route (D-27) how long to wait --
    /// counting a non-interactive route at the operation's default from the
    /// start. Once every asked route has answered (immediately, if none are
    /// interactive), the resolved delay is scheduled on `retry_timer`.
    fn enter_fault(self: &Arc<Self>, error: io::Error, operation: FaultOperation) {
        {
            let mut gate = lock(&self.gate);
            if *gate == ArmGate::TornDown {
                return;
            }
            *gate = ArmGate::Faulted;
        }

        log::warn!("windows-file-watcher: {operation:?} failed, recovering: {error}");

        // `routes` stays locked across installing `self.fault` and sending
        // every `RetryQuestion`: an answer arriving between those two steps
        // would find `self.fault` not yet naming that watch as awaiting
        // (D-27), so `self.answer` would silently discard it and this
        // interactive watch would wait forever for a question that was
        // already asked and already answered.
        let routes = lock(&self.routes);
        let mut awaiting = HashSet::new();
        for route in routes.values() {
            if route.report_liveness {
                let _ = route
                    .sink
                    .send(Notification::Suspended { watch: route.watch });
            }
            if route.retry == RetryMode::Interactive && route.fault_slot.is_some() {
                awaiting.insert(route.watch);
            }
        }

        let nobody_asked = awaiting.is_empty();
        *lock(&self.fault) = Some(FaultState {
            operation,
            awaiting,
            earliest: operation.default_delay(),
        });

        for route in routes.values() {
            if route.retry == RetryMode::Interactive
                && let Some(slot) = &route.fault_slot
            {
                slot.send(Notification::RetryQuestion {
                    watch: route.watch,
                    operation,
                });
            }
        }
        drop(routes);

        if nobody_asked {
            self.resolve_and_schedule(operation.default_delay());
        }
    }

    /// Record an interactive route's answer to the current fault's question
    /// (D-27), if there is one outstanding for it. A no-op otherwise: the fault
    /// may already have resolved, or `watch` may never have been asked.
    fn answer(&self, watch: WatchId, delay: Option<Duration>) {
        let resolved = {
            let mut fault = lock(&self.fault);
            let Some(state) = fault.as_mut() else {
                return;
            };
            if !state.awaiting.remove(&watch) {
                return;
            }
            let candidate = delay.unwrap_or_else(|| state.operation.default_delay());
            if candidate < state.earliest {
                state.earliest = candidate;
            }
            state.awaiting.is_empty().then_some(state.earliest)
        };
        if let Some(delay) = resolved {
            self.resolve_and_schedule(delay);
        }
    }

    /// Arm `retry_timer` for the resolved delay, clamped to the floor (D-27).
    fn resolve_and_schedule(&self, delay: Duration) {
        if let Some(timer) = self.retry_timer.get() {
            timer.set_after(clamp(delay));
        }
    }

    /// `retry_timer`'s callback: attempt one re-establishment.
    ///
    /// Reopens the directory from its original path (a live handle cannot be
    /// recovered once its target is gone, but the path can still be retried),
    /// then arms a read on it. An open failure that is retryable (D-22)
    /// re-enters the fault loop as an open-class fault; a permanent one is the
    /// one edge that does not (`stopped`). An arm failure after a successful
    /// open re-enters the fault loop as an arm-class fault.
    fn retry_reestablish(self: &Arc<Self>) {
        if *lock(&self.gate) == ArmGate::TornDown {
            return;
        }
        match DirectoryHandle::open(&self.path) {
            Ok(handle) => match self.reopen(handle) {
                Ok(()) => self.resolve_fault_success(),
                Err(error) => self.enter_fault(error, FaultOperation::Arm),
            },
            Err(open_error) => {
                if open_error.failure().is_retryable() {
                    self.enter_fault(io::Error::other(open_error), FaultOperation::Open);
                } else {
                    self.record_stop(io::Error::other(open_error));
                }
            }
        }
    }

    /// Clear fault state after a successful re-establishment and tell every
    /// route: the gap may have hidden changes (unconditional, like any other
    /// desync, D-12), and -- opt-in (D-13) -- that the watch resumed and which
    /// tier it resumed in.
    fn resolve_fault_success(&self) {
        *lock(&self.fault) = None;
        log::info!("windows-file-watcher: recovery succeeded, re-established");
        let mode = self.mode();
        // The desync is published *before* Resumed/Established: their own
        // documented contract ("a Desync always precedes or accompanies
        // this") means a client must be told to re-scan the gap before being
        // told it can trust incremental changes again, never the reverse.
        self.publish(DecodedBatch::Desync(DesyncCause::Reestablished));
        let routes = lock(&self.routes);
        for route in routes.values() {
            if route.report_liveness {
                let _ = route
                    .sink
                    .send(Notification::Resumed { watch: route.watch });
                let _ = route.sink.send(Notification::Established {
                    watch: route.watch,
                    mode,
                });
            }
        }
    }

    /// Which tier is currently servicing this directory (D-13/D-17). Detailed
    /// until an endpoint is installed, matching a not-yet-armed watcher's
    /// eventual default.
    fn mode(&self) -> WatchMode {
        match lock(&self.endpoint).as_ref() {
            Some(Endpoint::Coarse(_)) => WatchMode::Coarse,
            _ => WatchMode::Detailed,
        }
    }

    /// Tear down whichever endpoint is currently installed, fully -- cancelled
    /// or disarmed, then no operation outstanding and no callback executing --
    /// before anything touches a new one, the same ordering `stop()` relies on
    /// (D-23). Not held under the gate lock: both `run_down` and
    /// `stop_and_drain` can block.
    fn teardown_endpoint(&self) {
        // Taken and dropped *before* the match: a `match lock(...).take() {
        // ... }` scrutinee is a temporary whose lifetime extends across the
        // whole match, keeping the mutex held while `run_down`/`stop_and_drain`
        // block below -- exactly what the comment above says must not happen.
        let endpoint = lock(&self.endpoint).take();
        match endpoint {
            Some(Endpoint::Detailed(old)) => {
                let _ = old.cancel_all();
                old.run_down();
            }
            Some(Endpoint::Coarse(old)) => old.stop_and_drain(),
            None => {}
        }
    }

    /// Build a detailed endpoint over `handle` and install it. Does not arm it
    /// -- call [`WatcherInner::arm`] afterward, which is what actually reveals
    /// an unsupported-class failure (D-17/M6.3): `CreateThreadpoolIo` succeeding
    /// says nothing about whether `ReadDirectoryChangesW` itself is supported.
    ///
    /// # Errors
    ///
    /// Returns the error from binding the handle to the pool.
    fn establish_detailed(self: &Arc<Self>, handle: DirectoryHandle) -> io::Result<()> {
        // The callback holds a `Weak`, never a strong reference, for the same
        // cycle-avoidance reason as every other callback this watcher installs.
        let weak = Arc::downgrade(self);
        // SAFETY: `handle` was opened the same way every `DirectoryHandle` is
        // (see its module), which is the association this endpoint requires,
        // and ownership transfers here exclusively.
        let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(handle.into_handle()) };
        let io = ThreadpoolIo::new(
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
        *lock(&self.endpoint) = Some(Endpoint::Detailed(io));
        Ok(())
    }

    /// Open a coarse handle over `self.path` and install it. Does not arm it --
    /// call [`WatcherInner::arm`] afterward. The universal floor (D-17):
    /// reached only after a detailed arm reports an unsupported-class failure,
    /// or when `force_coarse` (M6.4's test seam) says to skip detailed
    /// entirely.
    ///
    /// # Errors
    ///
    /// Returns the error from opening the coarse handle or binding it to the
    /// pool.
    fn establish_coarse(self: &Arc<Self>) -> io::Result<()> {
        let subtree = lock(&self.routes)
            .values()
            .any(|route| route.scope.needs_kernel_subtree());
        let coarse =
            CoarseHandle::open(&self.path, subtree, self.filter).map_err(io::Error::other)?;
        let weak = Arc::downgrade(self);
        // SAFETY: `coarse` is a live `FindFirstChangeNotification` handle,
        // transferred exclusively, and is never touched again except through
        // the returned `WaitableHandle`.
        let waitable = unsafe { coarse.into_waitable() };
        let wait = ThreadpoolWait::new(
            waitable,
            move |activation: &WaitActivation<'_>| {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                inner.on_activation(activation);
            },
            None,
        )?;
        *lock(&self.endpoint) = Some(Endpoint::Coarse(wait));
        Ok(())
    }

    /// (Re-)establish, choosing between detailed and coarse (D-17/M6.3).
    ///
    /// Reopens the directory rather than cancelling and resubmitting on the
    /// same handle -- widening (M4.4) was the first thing this served, and
    /// re-establishment (M5.1) and a tier downgrade (M6.3) reuse the same
    /// mechanism: tear down whatever is installed, install something new.
    /// Detailed reads on the same handle were measured not to pick up a
    /// widened `bWatchSubtree`; a fresh `CreateFileW` does not have that
    /// problem, and a coarse handle cannot be reconfigured at all (its
    /// `bWatchSubtree` is fixed at open) so it needs exactly the same
    /// treatment.
    ///
    /// Mode is re-resolved on every call: detailed is attempted first (unless
    /// `force_coarse`, M6.4's test seam, says to skip it), and only an
    /// unsupported-class failure (`ERROR_INVALID_FUNCTION`/`ERROR_NOT_SUPPORTED`,
    /// D-17) falls back to coarse; every other failure is rearm-and-retry
    /// (D-15) and propagates to the caller unchanged.
    ///
    /// A no-op when there is nothing live to (re-)establish: [`ArmGate::TornDown`]
    /// has nothing left to reopen. Called both from [`ArmGate::Open`] (widening
    /// or downgrading for a new route, see [`DirectoryWatcher::add_route`]) and
    /// from [`ArmGate::Faulted`] (a re-establish attempt, see
    /// [`WatcherInner::retry_reestablish`]); either way it takes over the gate
    /// unconditionally except for `TornDown`.
    ///
    /// # Errors
    ///
    /// Returns the error from establishing or arming whichever tier was
    /// settled on.
    fn reopen(self: &Arc<Self>, handle: DirectoryHandle) -> io::Result<()> {
        {
            let mut gate = lock(&self.gate);
            if *gate == ArmGate::TornDown {
                return Ok(());
            }
            // Transient and self-resolving: cleared by the eventual `arm()`
            // call below, or left as `TornDown` if teardown wins the race.
            *gate = ArmGate::Reopening;
        }

        self.teardown_endpoint();

        if self.force_coarse.load(Ordering::Relaxed) {
            drop(handle);
        } else {
            self.establish_detailed(handle)?;
            match self.arm() {
                Ok(()) => return Ok(()),
                Err(error) if classify(&error) == OpenFailure::Unsupported => {
                    log::warn!(
                        "windows-file-watcher: detailed watching unsupported, downgrading to coarse: {error}"
                    );
                    self.teardown_endpoint();
                }
                Err(error) => return Err(error),
            }
        }

        self.establish_coarse()?;
        self.arm()
    }

    /// Record the *permanent* failure that stopped this watcher for good, and
    /// tell every route the change stream has a hole in it.
    ///
    /// Reached only from [`WatcherInner::retry_reestablish`], when a
    /// re-establish attempt's own open fails in a way D-22 classifies as
    /// permanent -- the one edge D-14's "no terminal state" does not cover,
    /// because retrying would spin forever against a target that can never
    /// become watchable again. Every other failure goes through
    /// [`WatcherInner::enter_fault`] instead.
    fn record_stop(&self, error: io::Error) {
        let mut stopped = lock(&self.stopped);
        if stopped.is_none() {
            *stopped = Some(error);
            drop(stopped);
            *lock(&self.fault) = None;
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
    /// `path` is kept for re-establishment (M5.1): a live handle cannot be
    /// recovered once its target is gone, but the path can still be retried.
    ///
    /// # Errors
    ///
    /// Returns the error from establishing or arming whichever tier the
    /// directory settles on (D-17), together with `route` handed back so a
    /// caller whose open just succeeded does not also lose the route's
    /// standing fault-question reservation (D-27/D-28) to a same-moment arm
    /// failure.
    pub(crate) fn start(
        directory: DirectoryHandle,
        path: PathBuf,
        route: Route,
    ) -> Result<Self, (io::Error, Route)> {
        Self::start_with(directory, path, DEFAULT_BUFFER_BYTES, route)
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
        path: PathBuf,
        buffer_bytes: usize,
        route: Route,
    ) -> Result<Self, (io::Error, Route)> {
        Self::start_inner(directory, path, buffer_bytes, route, false)
    }

    /// As [`start`](Self::start), but skipping the detailed attempt entirely
    /// and establishing coarse from the start (M6.4's test seam), regardless of
    /// what the underlying volume actually supports.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start).
    #[cfg(test)]
    pub(crate) fn start_forcing_coarse(
        directory: DirectoryHandle,
        path: PathBuf,
        route: Route,
    ) -> Result<Self, (io::Error, Route)> {
        Self::start_inner(directory, path, DEFAULT_BUFFER_BYTES, route, true)
    }

    /// Shared constructor body: build the resident state and its callback
    /// machinery, then establish the first read or wait through
    /// [`WatcherInner::reopen`] -- the same tier-choosing path used for every
    /// later widen, re-establish, or downgrade.
    ///
    /// On any failure, `route` is reclaimed from `inner.routes` (it is still
    /// the map's only entry) and returned alongside the error rather than
    /// dropped with the rest of `inner` -- see [`start`](Self::start)'s docs.
    fn start_inner(
        directory: DirectoryHandle,
        path: PathBuf,
        buffer_bytes: usize,
        route: Route,
        force_coarse: bool,
    ) -> Result<Self, (io::Error, Route)> {
        let watch = route.watch;
        let initial_sink = route.sink.clone();
        let mut routes = HashMap::new();
        routes.insert(watch, route);

        let inner = Arc::new(WatcherInner {
            path,
            endpoint: Mutex::new(None),
            resume_work: OnceLock::new(),
            retry_timer: OnceLock::new(),
            filter: ALL_NOTIFY_FILTERS,
            buffer_bytes,
            routes: Mutex::new(routes),
            gate: Mutex::new(ArmGate::Open),
            fault: Mutex::new(None),
            stopped: Mutex::new(None),
            force_coarse: AtomicBool::new(force_coarse),
        });

        // Pulls this constructor's one route back out of `inner.routes` so a
        // caller can recover it (and the standing reservation it may carry)
        // instead of losing it to `inner`'s drop.
        let reclaim_route = |inner: &Arc<WatcherInner>| -> Route {
            lock(&inner.routes)
                .remove(&watch)
                .expect("the route inserted above is still the map's only entry")
        };

        // As with the completion callback below, a `Weak` rather than a strong
        // reference: the work object lives in `inner`, so a strong one would be
        // a cycle, and a failed upgrade during teardown is exactly the
        // suppression that lets rundown converge.
        let resuming = Arc::downgrade(&inner);
        let resume_work = match ThreadpoolWork::new(
            move || {
                if let Some(inner) = resuming.upgrade() {
                    inner.resume_arm();
                }
            },
            None,
        ) {
            Ok(work) => work,
            Err(error) => return Err((error, reclaim_route(&inner))),
        };
        inner
            .resume_work
            .set(resume_work)
            .unwrap_or_else(|_| unreachable!("the resume work is set exactly once, here"));

        // Mirrors `resume_work`: a `Weak`, so a failed upgrade during teardown
        // suppresses a retry that fires after the watcher is gone. Set once here
        // and re-armed (never recreated) across however many faults this
        // watcher lives through (M5.1).
        let retrying = Arc::downgrade(&inner);
        let retry_timer = match ThreadpoolTimer::new(
            move |_firing| {
                if let Some(inner) = retrying.upgrade() {
                    inner.retry_reestablish();
                }
            },
            None,
        ) {
            Ok(timer) => timer,
            Err(error) => return Err((error, reclaim_route(&inner))),
        };
        inner
            .retry_timer
            .set(retry_timer)
            .unwrap_or_else(|_| unreachable!("the retry timer is set exactly once, here"));

        // Registered before the first arm, so a watcher that is backpressured
        // from the outset is still prodded when the client drains.
        initial_sink.register_resume(&inner);

        if let Err(error) = inner.reopen(directory) {
            return Err((error, reclaim_route(&inner)));
        }
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
    /// A reopen failure enters the fault loop (D-15's rearm-and-retry
    /// classification) rather than stopping the watcher, reported to every
    /// route it currently serves rather than only the one that triggered it,
    /// since they now share one endpoint.
    pub(crate) fn add_route(&self, route: Route, fresh_handle: DirectoryHandle) {
        let widen = route.scope.needs_kernel_subtree();
        let watch = route.watch;
        let sink = route.sink.clone();
        let already_recursive = {
            let mut routes = lock(&self.inner.routes);
            let already = routes
                .values()
                .any(|existing| existing.scope.needs_kernel_subtree());
            routes.insert(watch, route);
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
            self.inner.enter_fault(error, FaultOperation::Arm);
        }
    }

    /// Remove a subscription, returning how many routes remain.
    ///
    /// The caller tears this watcher down entirely once this reaches zero;
    /// removing the route itself never requires re-arming (see
    /// [`DirectoryWatcher::add_route`]).
    ///
    /// Cancellation from mid-fault (M5.5): a route awaiting an answer to the
    /// current fault's question that is removed here is treated as if it had
    /// just answered with a decline (it counts at the operation's default,
    /// then is simply not asked again) -- it is leaving, not answering, but the
    /// effect on the remaining reduction is identical, and if it was the last
    /// one still awaited this resolves and schedules the retry immediately
    /// rather than leaving the watcher waiting on an answer that can now never
    /// arrive.
    pub(crate) fn remove_route(&self, watch: WatchId) -> usize {
        let remaining = {
            let mut routes = lock(&self.inner.routes);
            routes.remove(&watch);
            routes.len()
        };
        let resolved = {
            let mut fault = lock(&self.inner.fault);
            match fault.as_mut() {
                Some(state) => {
                    if state.awaiting.remove(&watch) && state.awaiting.is_empty() {
                        Some(state.earliest)
                    } else {
                        None
                    }
                }
                None => None,
            }
        };
        if let Some(delay) = resolved {
            self.inner.resolve_and_schedule(delay);
        }
        remaining
    }

    /// Answer this watcher's current fault question on behalf of `watch`
    /// (D-27/M5.3), if one is outstanding. A no-op otherwise.
    pub(crate) fn answer(&self, watch: WatchId, delay: Option<Duration>) {
        self.inner.answer(watch, delay);
    }

    /// The failure that stopped this watcher re-arming, if any. Only ever set
    /// by a *permanent* re-establish failure (D-22); a watcher that is merely
    /// recovering (`is_faulted`) reports nothing here.
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
    /// A backpressured, reopening, or faulted watcher counts: each is paused,
    /// not stopped, and resumes on its own (D-14/D-29). D-31 is why the
    /// distinction is observable at all -- a paused watcher is otherwise
    /// indistinguishable from a quiet directory.
    pub fn is_watching(&self) -> bool {
        lock(&self.inner.stopped).is_none() && self.gate() != ArmGate::TornDown
    }

    /// Whether this watcher is currently working to re-establish itself
    /// (D-31/M5.6): asking, awaiting an answer, or backed off. Distinct from a
    /// permanent stop ([`DirectoryWatcher::stop_reason`]) and from a mere
    /// backpressure pause.
    #[must_use]
    pub fn is_faulted(&self) -> bool {
        lock(&self.inner.fault).is_some()
    }

    /// Which tier is currently servicing this directory (D-13/D-17).
    #[must_use]
    pub(crate) fn mode(&self) -> WatchMode {
        self.inner.mode()
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

        // Stops any further retry attempt and, if one is in flight, blocks until
        // it has fully returned -- so by the time this returns, `self.inner`'s
        // endpoint is whatever a completed `reopen` last left it as, and is
        // safe for teardown to act on directly (see `reopen`'s own
        // teardown-race handling).
        if let Some(timer) = self.inner.retry_timer.get() {
            timer.stop_and_drain();
        }

        // With the gate closed, nothing new can be submitted, so tearing down
        // whichever endpoint is installed retires everything outstanding and
        // rundown converges (D-20). Safe to repeat: `teardown_endpoint` takes
        // the endpoint, so a second call finds nothing there.
        self.inner.teardown_endpoint();
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
