// Copyright (c) 2026 Mike Grier
//! The watcher and its roster of registered files.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wtf_string::Wtf16Str;

use crate::sq::{self, SqCapacityError, SqDrain, SqReader, SqWriter};
use crate::{HeldFile, OpenError};

#[cfg(test)]
mod tests;

/// A set of files whose lengths are watched, each registered with a caller value `T`.
///
/// Registering opens the file at once and holds it ([`HeldFile`]); the [`Registration`] it returns
/// ends the registration when it drops. There is no uniqueness and no lookup by name: registering
/// one path twice gives two independent registrations.
///
/// It owns the submission queue ([`SqEntry`](crate::SqEntry)) the caller drains with
/// [`drain`](Self::drain); the queue lives exactly as long as the watcher.
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
        // The lock is never held across user code, so a poisoned roster is still consistent.
        self.roster.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T: Clone> Watcher<T> {
    /// A watcher with no registrations, and a submission queue that holds `sq_capacity` entries.
    ///
    /// # Errors
    ///
    /// [`SqCapacityError`] unless `sq_capacity` is a power of two of at least two.
    pub fn new(sq_capacity: usize) -> Result<Self, SqCapacityError> {
        let (writer, reader) = sq::queue(sq_capacity)?;
        Ok(Self {
            shared: Arc::new(Shared {
                roster: Mutex::new(Roster {
                    next_id: 0,
                    entries: BTreeMap::new(),
                }),
                writer: Mutex::new(writer),
                reader: Mutex::new(reader),
            }),
        })
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
        expect(dead_code, reason = "the poll (TW-2.4) writes through it")
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
