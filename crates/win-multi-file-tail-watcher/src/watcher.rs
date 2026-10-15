// Copyright (c) 2026 Mike Grier
//! The watcher and its roster of registered files.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wtf_string::Wtf16Str;

use crate::{HeldFile, OpenError};

#[cfg(test)]
mod tests;

/// A set of files whose lengths are watched, each registered with a caller value `T`.
///
/// Registering opens the file at once and holds it ([`HeldFile`]); the [`Registration`] it returns
/// ends the registration when it drops. There is no uniqueness and no lookup by name: registering
/// one path twice gives two independent registrations.
#[derive(Debug)]
pub struct Watcher<T: Clone> {
    shared: Arc<Shared<T>>,
}

#[derive(Debug)]
struct Shared<T> {
    roster: Mutex<Roster<T>>,
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
    /// A watcher with no registrations.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                roster: Mutex::new(Roster {
                    next_id: 0,
                    entries: BTreeMap::new(),
                }),
            }),
        }
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

impl<T: Clone> Default for Watcher<T> {
    fn default() -> Self {
        Self::new()
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
