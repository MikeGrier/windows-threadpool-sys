// Copyright (c) 2026 Mike Grier
//! The bounded completion ring: everything a receiver ever observes.
//!
//! # Reserved is guaranteed, unreserved is best-effort
//!
//! Two very different things travel this ring. An entry is *recoverable* under
//! pressure -- the enumeration that would have produced it simply has not parsed
//! it yet, and will when there is room -- while a terminal outcome is not: a
//! receiver that never learns an enumeration ended waits for something that
//! already happened.
//!
//! Rather than decide that per message at the enqueue, reliability is a property
//! of **reserved capacity**. An enumeration takes its terminal slot before it is
//! allowed to start, so [`TerminalSlot::send`] cannot fail. Entries reserve
//! nothing and go through [`CompletionRing::try_send_entry`], which is allowed
//! to refuse.
//!
//! # Nothing is ever dropped
//!
//! A refused entry is handed straight back to its caller, because the caller has
//! not yet committed to having produced it. That is the whole backpressure
//! mechanism: an enumeration asks whether there is room *before* it parses the
//! next record, and if there is not, it keeps its native buffer and its record
//! cursor and yields the worker. Nothing is dropped, nothing is latched, and
//! nothing blocks a thread-pool thread.
//!
//! # One slot is never reservable
//!
//! Terminal reservations may never consume the whole ring: if they could, a
//! session with `capacity` active enumerations would have no room for a single
//! entry, and every one of them would be permanently backpressured. The ring
//! therefore always keeps one slot out of reach of reservations, which is also
//! why a capacity of one cannot support this contract at all.
//!
//! # The doorbell
//!
//! The receiver hands out a manual-reset event so a client that already owns a
//! thread pool can integrate without dedicating a thread to a blocking receive.
//! Its invariant is one line: **signalled exactly when the receiver has
//! something to observe** -- a queued record, or the end of the stream. It is
//! re-established under the ring lock at the end of every mutation, which is the
//! same lock a receiver holds while deciding there is nothing to take, so there
//! is no gap in which a wakeup could be lost.
//!
//! The event is created on first request, so a client that only ever polls pays
//! for no kernel object.

use std::collections::VecDeque;
use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, FALSE, HANDLE};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, ResetEvent, SetEvent};
use windows_threadpool_sys::wait::WaitableHandle;

use crate::completion::{Completion, EnumerationId, TerminalOutcome};

/// The smallest completion-ring capacity that can satisfy the contract.
///
/// One slot is always held back from terminal reservations, so a ring of one
/// could never hold both an active enumeration's terminal and a single entry.
pub(crate) const MINIMUM_COMPLETION_CAPACITY: usize = 2;

/// The bounded ring plus the machinery that tells a receiver about it.
pub(crate) struct CompletionRing {
    state: Mutex<RingState>,
    arrived: Condvar,
    doorbell: OnceLock<WaitableHandle>,
}

struct RingState {
    queue: VecDeque<Completion>,
    capacity: usize,
    /// Terminal slots claimed but not yet filled.
    reserved: usize,
    /// Live session handles. While any remains, more enumerations may start.
    sessions: usize,
    /// Accepted enumerations that have not yet delivered their terminal.
    active: usize,
}

impl RingState {
    /// Slots that an entry could take: neither occupied nor reserved.
    fn data_room(&self) -> usize {
        self.capacity - self.queue.len() - self.reserved
    }

    /// Whether one more terminal reservation is admissible.
    ///
    /// Two conditions, and they are not the same one: there must be room for it
    /// now, and reservations must not grow to cover the whole ring even when it
    /// is empty.
    fn can_reserve(&self) -> bool {
        self.data_room() > 0 && self.reserved + 1 < self.capacity
    }

    /// Whether the stream has ended: nothing can produce another record.
    fn closed(&self) -> bool {
        self.sessions == 0 && self.active == 0
    }

    /// Whether a receiver has anything to observe.
    ///
    /// The end of the stream counts, or a client waiting on the doorbell would
    /// wait for a record that can never arrive.
    fn pending(&self) -> bool {
        !self.queue.is_empty() || self.closed()
    }
}

impl CompletionRing {
    /// Build a ring holding at most `capacity` records.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is below [`MINIMUM_COMPLETION_CAPACITY`]; callers
    /// validate and report that as an error before constructing a session.
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(
            capacity >= MINIMUM_COMPLETION_CAPACITY,
            "a completion ring must be able to hold one terminal and one entry"
        );
        Self {
            state: Mutex::new(RingState {
                queue: VecDeque::new(),
                capacity,
                reserved: 0,
                sessions: 1,
                active: 0,
            }),
            arrived: Condvar::new(),
            doorbell: OnceLock::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, RingState> {
        // A poisoned ring still describes exactly what it held: the panic that
        // poisoned it happened in a caller, not in this invariant.
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// The ring's bound.
    pub(crate) fn capacity(&self) -> usize {
        self.lock().capacity
    }

    /// How many records are queued right now.
    pub(crate) fn len(&self) -> usize {
        self.lock().queue.len()
    }

    /// Whether an entry would be accepted right now.
    ///
    /// An enumeration asks this *before* parsing its next record, which is what
    /// turns a full ring into a yield rather than a dropped entry.
    #[allow(dead_code, reason = "the native engine (M6) is the caller that asks")]
    pub(crate) fn has_data_room(&self) -> bool {
        self.lock().data_room() > 0
    }

    /// Whether the stream has ended.
    pub(crate) fn is_closed(&self) -> bool {
        self.lock().closed()
    }

    /// How many slots outstanding terminal reservations hold.
    #[cfg(test)]
    pub(crate) fn reserved(&self) -> usize {
        self.lock().reserved
    }

    /// Whether a receiver has anything to observe, which is exactly the
    /// doorbell's signalled state.
    #[cfg(test)]
    pub(crate) fn is_pending(&self) -> bool {
        self.lock().pending()
    }

    /// Register one more session handle.
    pub(crate) fn add_session(&self) {
        self.lock().sessions += 1;
    }

    /// Drop one session handle, ending the stream if it was the last producer.
    pub(crate) fn remove_session(&self) {
        {
            let mut state = self.lock();
            state.sessions -= 1;
            self.refresh_doorbell(&state);
        }
        self.arrived.notify_all();
    }

    /// Claim the slot in which one enumeration's terminal will be delivered.
    ///
    /// Taken before the enumeration is allowed to start, so the outcome it owes
    /// can always be reported. Returns `None` when the ring cannot spare a slot
    /// while still leaving one an entry could use.
    ///
    /// The slot owns a share of the ring rather than borrowing it, because it
    /// outlives the call and is parked in the registry the ring's own session
    /// holds -- a borrow would make that structure self-referential.
    pub(crate) fn reserve_terminal(
        self: &Arc<Self>,
        enumeration: EnumerationId,
    ) -> Option<TerminalSlot> {
        let mut state = self.lock();
        if !state.can_reserve() {
            return None;
        }
        state.reserved += 1;
        state.active += 1;
        Some(TerminalSlot {
            ring: Arc::clone(self),
            enumeration,
        })
    }

    /// Offer one entry, best-effort.
    ///
    /// # Errors
    ///
    /// Returns the entry unchanged when there is no room, so the caller can keep
    /// it and retry after the receiver makes progress.
    #[allow(
        clippy::result_large_err,
        reason = "handing the record back by value is the point: it is returned \
                  intact rather than boxed, reallocated, or dropped"
    )]
    pub(crate) fn try_send_entry(&self, record: Completion) -> Result<(), Completion> {
        {
            let mut state = self.lock();
            if state.data_room() == 0 {
                return Err(record);
            }
            state.queue.push_back(record);
            self.refresh_doorbell(&state);
        }
        self.arrived.notify_all();
        Ok(())
    }

    /// Take the next record, if one is queued.
    pub(crate) fn try_take(&self) -> Option<Completion> {
        let mut state = self.lock();
        let record = state.queue.pop_front();
        if record.is_some() {
            self.refresh_doorbell(&state);
        }
        record
    }

    /// Block until a record is available or the stream ends.
    pub(crate) fn take_blocking(&self, timeout: Option<Duration>) -> Option<Completion> {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        let mut state = self.lock();
        loop {
            if let Some(record) = state.queue.pop_front() {
                self.refresh_doorbell(&state);
                return Some(record);
            }
            if state.closed() {
                return None;
            }
            state = match deadline {
                None => self
                    .arrived
                    .wait(state)
                    .unwrap_or_else(|poison| poison.into_inner()),
                Some(deadline) => {
                    let remaining = deadline.checked_duration_since(Instant::now())?;
                    self.arrived
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(|poison| poison.into_inner())
                        .0
                }
            };
        }
    }

    /// The manual-reset event, created on first use.
    ///
    /// Created under the ring lock so its initial state cannot disagree with the
    /// ring it reports on: a client that asks for it after records have already
    /// arrived must find it signalled.
    pub(crate) fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        if self.doorbell.get().is_none() {
            let state = self.lock();
            if self.doorbell.get().is_none() {
                let event = WaitableHandle::event(true, state.pending())?;
                let _ = self.doorbell.set(event);
            }
        }
        Ok(self
            .doorbell
            .get()
            .expect("the doorbell was just created")
            .handle())
    }

    /// A duplicate of the doorbell that the caller owns, as a `ThreadpoolWait`
    /// requires.
    pub(crate) fn doorbell_owned(&self) -> io::Result<OwnedHandle> {
        duplicate(self.doorbell()?)
    }

    /// Re-establish the doorbell's one invariant.
    ///
    /// Called with the ring lock held at the end of every mutation, which is
    /// what closes the gap between "there is nothing to take" and "a wakeup was
    /// delivered".
    fn refresh_doorbell(&self, state: &RingState) {
        let Some(event) = self.doorbell.get() else {
            return;
        };
        let handle = event.handle().as_raw_handle() as HANDLE;
        // SAFETY: the handle is a live manual-reset event owned by this ring.
        unsafe {
            if state.pending() {
                SetEvent(handle);
            } else {
                ResetEvent(handle);
            }
        }
    }

    /// Give back a terminal reservation that was never used.
    fn release_reservation(&self) {
        {
            let mut state = self.lock();
            state.reserved -= 1;
            state.active -= 1;
            self.refresh_doorbell(&state);
        }
        self.arrived.notify_all();
    }

    /// Fill a terminal reservation.
    fn fill_reservation(&self, record: Completion) {
        {
            let mut state = self.lock();
            state.reserved -= 1;
            state.active -= 1;
            state.queue.push_back(record);
            self.refresh_doorbell(&state);
        }
        self.arrived.notify_all();
    }
}

/// One enumeration's claimed terminal slot.
///
/// Holding this is what makes the outcome infallible to deliver. Dropping it
/// unused returns the slot, which is what receiver abandonment does: no observer
/// remains, so no outcome is owed.
pub(crate) struct TerminalSlot {
    ring: Arc<CompletionRing>,
    enumeration: EnumerationId,
}

impl TerminalSlot {
    /// Deliver the outcome. Cannot fail: the room was taken up front.
    pub(crate) fn send(self, outcome: TerminalOutcome) {
        let record = Completion::Terminal {
            enumeration: self.enumeration,
            outcome,
        };
        self.ring.fill_reservation(record);
        // The slot has been consumed; `Drop` must not also release it.
        std::mem::forget(self);
    }
}

impl Drop for TerminalSlot {
    fn drop(&mut self) {
        self.ring.release_reservation();
    }
}

/// Duplicate a handle into one the caller owns.
fn duplicate(handle: BorrowedHandle<'_>) -> io::Result<OwnedHandle> {
    let mut copy: HANDLE = std::ptr::null_mut();
    // SAFETY: `handle` is live for the borrow, `copy` is a valid out-pointer,
    // and both process handles are the current-process pseudo handle.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle() as HANDLE,
            GetCurrentProcess(),
            &raw mut copy,
            0,
            FALSE,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `DuplicateHandle` succeeded, so `copy` owns a live handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(copy as _) })
}

#[cfg(test)]
mod tests;
