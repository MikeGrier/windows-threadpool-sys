// Copyright (c) 2026 Mike Grier
//! The watcher and its roster of registered files.

use std::collections::BTreeMap;
use std::os::windows::io::BorrowedHandle;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wtf_string::Wtf16Str;

use crate::doorbell::Doorbell;
use crate::sq::{self, SqDrain, SqReader, SqWriter};
use crate::{CreateError, HeldFile, OpenError};

#[cfg(test)]
mod tests;

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
/// dropping a [`Registration`], draining) deadlocks.
#[derive(Debug)]
pub struct Watcher<T: Clone> {
    shared: Arc<Shared<T>>,
}

#[derive(Debug)]
struct Shared<T> {
    roster: Mutex<Roster<T>>,
    // Held only to hand out clones, so no writer ever waits on this lock to write.
    writer: Mutex<SqWriter<T>>,
    reader: Mutex<SqReader<T>>,
    // Crate to caller: entries are ready, the SQ is full, or a poll is done.
    doorbell: Doorbell,
    // Caller to crate: the caller has made room in the SQ.
    resume: Doorbell,
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
}

impl<T> Shared<T> {
    fn roster(&self) -> MutexGuard<'_, Roster<T>> {
        // `T::clone` runs under the lock but only reads; nothing under it can leave the map
        // half-changed, so a poisoned roster is still consistent.
        self.roster.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T: Clone> Watcher<T> {
    /// A watcher with no registrations, and a submission queue that holds `sq_capacity` entries.
    ///
    /// # Errors
    ///
    /// [`CreateError::Capacity`] unless `sq_capacity` is a power of two of at least two.
    /// [`CreateError::Io`] when a doorbell cannot be created.
    pub fn new(sq_capacity: usize) -> Result<Self, CreateError> {
        let (writer, reader) = sq::queue(sq_capacity)?;
        Ok(Self {
            shared: Arc::new(Shared {
                roster: Mutex::new(Roster {
                    next_id: 0,
                    entries: BTreeMap::new(),
                }),
                writer: Mutex::new(writer),
                reader: Mutex::new(reader),
                doorbell: Doorbell::new()?,
                resume: Doorbell::new()?,
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
    #[cfg_attr(not(test), expect(dead_code, reason = "the poll (TW-3.1) rings it"))]
    pub(crate) fn ring_doorbell(&self) {
        self.shared.doorbell.ring();
    }

    /// Clear [`resume_doorbell`](Self::resume_doorbell).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "a resumed poll (TW-3.2) clears it")
    )]
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

    /// A writing handle for one thread of the crate's own.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the report pass (TW-2.7) writes through it")
    )]
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
        let file = HeldFile::open_at(path, offset)?;
        let mut roster = self.shared.roster();
        let id = roster.next_id;
        roster.next_id += 1;
        roster.entries.insert(id, Entry { value, file });
        drop(roster);
        Ok(Registration {
            shared: Arc::clone(&self.shared),
            id,
        })
    }

    /// How many registrations are live.
    #[must_use]
    pub fn registered(&self) -> usize {
        self.shared.roster().entries.len()
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
