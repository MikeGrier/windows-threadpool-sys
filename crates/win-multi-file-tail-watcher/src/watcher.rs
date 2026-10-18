// Copyright (c) 2026 Mike Grier
//! The watcher and its roster of registered files.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::os::windows::io::BorrowedHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use windows_overlapped_io_sys::EndpointObserver;
use wtf_string::Wtf16Str;

use crate::chunked::Chunked;
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink, NoDiagnostics, SharedSink};
use crate::doorbell::Doorbell;
use crate::sq::{self, SqDrain, SqReader, SqWriter};
use crate::{CreateError, HeldFile, OpenError};

#[cfg(test)]
mod tests;

mod engine;
mod poll;
mod probe;

use poll::{PollGate, Poller};

#[cfg(test)]
mod snapshot_tests;

/// How many roster entries the snapshot copies per lock acquisition (TW-D-14).
const SNAPSHOT_BATCH: usize = 256;

/// What the snapshot pass copies from a roster entry (TW-D-7).
#[derive(Debug)]
pub(crate) struct SnapshotItem {
    pub(crate) id: u64,
    pub(crate) last_known: u64,
    pub(crate) observer: EndpointObserver,
}

/// A set of files whose lengths are watched, each registered with a caller value `T`.
///
/// Registering opens the file at once and holds it ([`HeldFile`]); the [`Registration`] it returns
/// ends the registration when it drops. There is no uniqueness and no lookup by name: registering
/// one path twice gives two independent registrations.
///
/// It owns the submission queue ([`SqEntry`](crate::SqEntry)) the caller drains with
/// [`drain`](Self::drain); the queue lives exactly as long as the watcher. So do its two doorbells,
/// both manual-reset events: [`doorbell`](Self::doorbell), which the crate rings and the caller
/// waits on, and [`resume_doorbell`](Self::resume_doorbell), the other way.
///
/// `T::clone` runs while the watcher's internal lock is held, here in [`Registration::value`] and
/// when the crate reports a change. A `Clone` that calls back into the watcher (registering,
/// dropping a [`Registration`], draining) deadlocks. `T`'s `Drop` runs after the lock is released, so
/// it may do any of those, with one exception: a `T` is also dropped on a thread-pool callback of
/// the crate (the clone of an entry the queue refused is), so a `Drop` must not release the last
/// owner of the watcher, which waits for that callback when it is dropped.
///
/// `T::clone`, `T`'s `Drop` and the [`DiagnosticSink`](crate::DiagnosticSink) run on thread-pool
/// callbacks, and a panic that unwinds out of a callback aborts the process.
///
/// Dropping the watcher stops it as [`shutdown`](Self::shutdown) does, and then waits for the
/// crate's own callbacks: a poll that is running reaches its next step and returns, and a poll
/// parked on a full queue is not waited for. It never waits for poll-done to be written or read, so
/// it may be dropped without draining. When it returns no callback of the crate runs again.
#[derive(Debug)]
pub struct Watcher<T: Clone> {
    shared: Arc<Shared<T>>,
    // Made by the first `poll`; a watcher that is never polled makes none.
    poller: OnceLock<Poller>,
    // The callbacks' view of the shared state, not a watcher a caller holds: dropping it stops nothing.
    view: bool,
}

#[derive(Debug)]
struct Shared<T> {
    gate: PollGate,
    // Set by `shutdown`, for good (TW-D-9).
    stop: AtomicBool,
    roster: Mutex<Roster<T>>,
    // Held only to hand out clones, so no writer ever waits on this lock to write.
    writer: Mutex<SqWriter<T>>,
    reader: Mutex<SqReader<T>>,
    // Crate to caller: entries are ready, the SQ is full, or a poll is done.
    doorbell: Doorbell,
    // Caller to crate: the caller has made room in the SQ.
    resume: Doorbell,
    diagnostics: SharedSink,
}

// Ids only grow, so the roster's order is the order of registration and a position in it survives
// removals.
#[derive(Debug)]
struct Roster<T> {
    next_id: u64,
    entries: BTreeMap<u64, Entry<T>>,
}

#[derive(Debug)]
struct Entry<T> {
    value: T,
    file: HeldFile,
    // The length last reported for this registration (TW-D-4). It starts at the starting offset.
    last_known: u64,
    // The Win32 code that made this registration terminal (TW-D-13), set once the caller has been
    // told. A terminal entry is no longer probed or snapshotted, and stays until dropped.
    terminal: Option<u32>,
}

impl<T> Entry<T> {
    /// Note that this registration can never be probed again, `code` being why. Returns whether it
    /// did: `false` when it was already terminal, so the first code stays.
    fn mark_terminal(&mut self, code: u32) -> bool {
        if self.terminal.is_some() {
            return false;
        }
        self.terminal = Some(code);
        true
    }
}

impl<T> Shared<T> {
    // Whether `shutdown` has been called.
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn roster(&self) -> MutexGuard<'_, Roster<T>> {
        // `T::clone` runs under the lock but only reads; nothing under it can leave the map
        // half-changed, so a poisoned roster is still consistent.
        self.roster.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T: Clone> Drop for Watcher<T> {
    fn drop(&mut self) {
        // Set first, so a poll still running stops at its next step; the poller's fields then drain
        // the work item and the wait, which is where this blocks. A callback's view is also dropped
        // when a poller that could not be made is abandoned, and the watcher it served goes on.
        if !self.view {
            self.shared.stop.store(true, Ordering::Release);
        }
    }
}

impl<T: Clone> Watcher<T> {
    /// A watcher with no registrations, and a submission queue that holds `sq_capacity` entries.
    ///
    /// # Errors
    ///
    /// [`CreateError::Capacity`] unless `sq_capacity` is a power of two of at least two.
    /// [`CreateError::Allocation`] when the memory for the queue's slots cannot be allocated.
    /// [`CreateError::Io`] when a doorbell cannot be created.
    pub fn new(sq_capacity: usize) -> Result<Self, CreateError> {
        Self::with_diagnostics(sq_capacity, Arc::new(NoDiagnostics))
    }

    /// As [`new`](Self::new), reporting what the watcher observes about itself to `diagnostics`.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_diagnostics(
        sq_capacity: usize,
        diagnostics: Arc<dyn DiagnosticSink>,
    ) -> Result<Self, CreateError> {
        let (writer, reader) = sq::queue(sq_capacity)?;
        Ok(Self {
            poller: OnceLock::new(),
            view: false,
            shared: Arc::new(Shared {
                gate: PollGate::new(),
                stop: AtomicBool::new(false),
                roster: Mutex::new(Roster {
                    next_id: 0,
                    entries: BTreeMap::new(),
                }),
                writer: Mutex::new(writer),
                reader: Mutex::new(reader),
                doorbell: Doorbell::new()?,
                resume: Doorbell::new()?,
                diagnostics: SharedSink::new(diagnostics),
            }),
        })
    }

    /// The event the crate rings when entries are ready, the submission queue is full, or a poll
    /// is done. It is rung until [`reset_doorbell`](Self::reset_doorbell), which releases every
    /// wait on it meanwhile.
    ///
    /// The caller learns which of the three by reading the queue. Reset first and then drain, so
    /// an entry written after the reset rings it again.
    #[must_use]
    pub fn doorbell(&self) -> BorrowedHandle<'_> {
        self.shared.doorbell.handle()
    }

    /// Clear [`doorbell`](Self::doorbell). Does nothing if it is not rung.
    pub fn reset_doorbell(&self) {
        self.shared.doorbell.reset();
    }

    /// The event the caller rings, with [`ring_resume`](Self::ring_resume), to say it has made room
    /// in the submission queue. The crate waits on it.
    #[must_use]
    pub fn resume_doorbell(&self) -> BorrowedHandle<'_> {
        self.shared.resume.handle()
    }

    /// Ring [`resume_doorbell`](Self::resume_doorbell).
    pub fn ring_resume(&self) {
        self.shared.resume.ring();
    }

    /// Ring [`doorbell`](Self::doorbell).
    pub(crate) fn ring_doorbell(&self) {
        self.shared.doorbell.ring();
    }

    /// Clear [`resume_doorbell`](Self::resume_doorbell).
    pub(crate) fn reset_resume(&self) {
        self.shared.resume.reset();
    }

    /// The entries in the submission queue, oldest first, until there are none.
    ///
    /// Taking an entry makes room for another. Only one drain is live at a time: a second waits
    /// for the first to drop. It never waits for the crate's writing.
    pub fn drain(&self) -> SqDrain<'_, T> {
        SqDrain(
            self.shared
                .reader
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// How many entries the submission queue holds when full.
    #[must_use]
    pub fn sq_capacity(&self) -> usize {
        self.shared
            .reader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .capacity()
    }

    /// Report an event to the watcher's sink if it wants `kind`; `make` builds the event only then.
    pub(crate) fn diagnose(&self, kind: DiagnosticKind, make: impl FnOnce() -> Diagnostic) {
        self.shared.diagnostics.emit(kind, make);
    }

    /// A writing handle for one thread of the crate's own.
    pub(crate) fn sq_writer(&self) -> SqWriter<T> {
        self.shared
            .writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Register the file at `path`, taking `offset` as where the caller's reading starts, and move
    /// `value` into the registration.
    ///
    /// # Errors
    ///
    /// As [`HeldFile::open_at`]. `value` is dropped with the error, so a caller who still wants it
    /// keeps a clone.
    pub fn register(
        &self,
        path: &Wtf16Str,
        offset: u64,
        value: T,
    ) -> Result<Registration<T>, OpenError> {
        self.register_with(path, offset, |_, _| value)
    }

    /// As [`register`](Self::register), the value made by `make` from the file once it is open and
    /// the id the registration will have. `make` runs with the roster lock held, so it must not
    /// call back into the watcher.
    ///
    /// # Errors
    ///
    /// As [`register`](Self::register); `make` is not called then.
    pub(crate) fn register_with(
        &self,
        path: &Wtf16Str,
        offset: u64,
        make: impl FnOnce(&HeldFile, u64) -> T,
    ) -> Result<Registration<T>, OpenError> {
        let file = HeldFile::open_at(path, offset)?;
        let mut roster = self.shared.roster();
        let id = roster.next_id;
        roster.next_id += 1;
        let value = make(&file, id);
        let last_known = file.start_offset();
        roster.entries.insert(
            id,
            Entry {
                value,
                file,
                last_known,
                terminal: None,
            },
        );
        drop(roster);
        Ok(Registration {
            shared: Arc::clone(&self.shared),
            id,
        })
    }

    /// Append to `out` the id, last known length and an observer of every registration that can
    /// still be probed, in registration order, taking the roster lock once per batch and never
    /// across the whole roster. A terminal registration is skipped.
    ///
    /// A registration made while the pass runs is seen if the pass has not yet reached the end of
    /// the roster; one dropped is skipped if the pass has not yet reached it, and its observer
    /// keeps the file open if it has.
    pub(crate) fn snapshot(&self, out: &mut Chunked<SnapshotItem>) {
        self.snapshot_batches(out, SNAPSHOT_BATCH, || {});
    }

    // `between` runs after each full batch with the lock released, which is where a later stop
    // check can go, and where a test can change the roster.
    fn snapshot_batches(
        &self,
        out: &mut Chunked<SnapshotItem>,
        batch: usize,
        mut between: impl FnMut(),
    ) {
        assert!(batch > 0, "a batch of nothing never finishes");
        let mut after = None;
        loop {
            // A shutdown ends the copy at its next batch.
            if self.shared.stopping() {
                return;
            }
            // Counts entries looked at, skipped or not: a batch of terminal ones is a full batch.
            let scanned = {
                let roster = self.shared.roster();
                let from = after.map_or(Bound::Unbounded, Bound::Excluded);
                let mut scanned = 0;
                for (&id, entry) in roster.entries.range((from, Bound::Unbounded)).take(batch) {
                    if entry.terminal.is_none() {
                        out.push(SnapshotItem {
                            id,
                            last_known: entry.last_known,
                            observer: entry.file.observer(),
                        });
                    }
                    after = Some(id);
                    scanned += 1;
                }
                scanned
            };
            if scanned < batch {
                return;
            }
            between();
        }
    }

    /// Note that the registration `id` can never be probed again, `code` being why. Returns whether
    /// it did: `false` when there is no such registration, or it was already terminal.
    pub(crate) fn mark_terminal(&self, id: u64, code: u32) -> bool {
        self.shared
            .roster()
            .entries
            .get_mut(&id)
            .is_some_and(|entry| entry.mark_terminal(code))
    }

    /// How many registrations are live.
    #[must_use]
    pub fn registered(&self) -> usize {
        self.shared.roster().entries.len()
    }

    /// Run a poll on the calling thread, to completion, for tests: the SQ must have room for it.
    #[cfg(test)]
    pub(crate) fn poll_here(&self) {
        assert_eq!(
            engine::Engine::new(self).run(self),
            engine::RunOutcome::Complete,
            "the SQ had no room for the poll"
        );
    }

    /// Stop the watcher for good: no poll starts after this returns.
    ///
    /// It sets a flag and returns, so it never blocks, may be called from a callback for
    /// [`doorbell`](Self::doorbell), and may be called again. It does not wait for anything: a poll
    /// already running is not drained by it. Dropping the watcher is what waits.
    ///
    /// A poll in flight abandons what it has not yet done and still ends with its poll-done entry,
    /// which waits for room in a full queue like any entry.
    pub fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }

    /// A handle that makes the same request as [`shutdown`](Self::shutdown) without a borrow of the
    /// watcher, for an owner that keeps the watcher under a lock a request must not wait for.
    pub(crate) fn stop_handle(&self) -> StopHandle<T> {
        StopHandle(Arc::clone(&self.shared))
    }
}

/// See [`Watcher::stop_handle`]. It keeps the watcher's shared state, files included, alive, so an
/// owner drops it with the watcher.
pub(crate) struct StopHandle<T>(Arc<Shared<T>>);

impl<T> StopHandle<T> {
    /// As [`Watcher::shutdown`]: set a flag, and return.
    pub(crate) fn request(&self) {
        self.0.stop.store(true, Ordering::Release);
    }
}

/// One file's registration with a [`Watcher`]. Dropping it unregisters the file.
///
/// It does not wait for anything: unregistering is legal at any time. It may outlive the
/// [`Watcher`] it came from.
#[derive(Debug)]
#[must_use = "dropping a registration unregisters it at once"]
pub struct Registration<T: Clone> {
    shared: Arc<Shared<T>>,
    id: u64,
}

impl<T: Clone> Registration<T> {
    /// This registration's number within its watcher: unique, handed out in order, never reused.
    /// [`Diagnostic`] events name a registration by it.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// A clone of the value this registration was made with.
    #[must_use]
    pub fn value(&self) -> T {
        self.with_entry(|entry| entry.value.clone())
    }

    /// The starting offset this registration was made with.
    #[must_use]
    pub fn start_offset(&self) -> u64 {
        self.with_entry(|entry| entry.file.start_offset())
    }

    #[cfg(test)]
    pub(crate) fn last_known(&self) -> u64 {
        self.with_entry(|entry| entry.last_known)
    }

    #[cfg(test)]
    pub(crate) fn terminal_code(&self) -> Option<u32> {
        self.with_entry(|entry| entry.terminal)
    }

    fn with_entry<R>(&self, read: impl FnOnce(&Entry<T>) -> R) -> R {
        let roster = self.shared.roster();
        read(
            roster
                .entries
                .get(&self.id)
                .expect("an entry stays in the roster until its registration drops"),
        )
    }
}

impl<T: Clone> Drop for Registration<T> {
    fn drop(&mut self) {
        let removed = self.shared.roster().entries.remove(&self.id);
        // Dropped here, after the lock is released: closing the file and dropping `T` are not
        // done under it.
        drop(removed);
    }
}
