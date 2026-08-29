// Copyright (c) 2026 Mike Grier
//! Epoch bookkeeping and group commit (M13.3).
//!
//! This is the construction from "Durability on the ring" in the crate's
//! `DESIGN-NOTES.md`, and it is what turns [`crate::contract`]'s guarantee
//! from a sentence into a value a caller can ask about:
//!
//! 1. Records stream into whatever epoch is currently **open**, carrying no
//!    durability flag of their own.
//! 2. Closing epoch *N* pushes **one covering flush**, whose `UserData` is
//!    *N*'s identity for the rest of its life.
//! 3. When that flush's completion is observed, every epoch `<= N` is durable.
//! 4. Callers wait on epochs, never on records.
//!
//! One expensive operation amortized over many records. Step 2's barrier is
//! not decoration: without [`FlushCoverage::CoversPrecedingOperations`] the
//! flush is an ordinary operation racing the writes it is supposed to cover
//! (D-23), and step 3 would be false.
//!
//! # What the barrier costs, and why this module says so out loud
//!
//! [`FlushCoverage::CoversPrecedingOperations`] is a **ring-wide** barrier
//! (D-24): it waits for every operation outstanding on the ring, and holds
//! back every operation pushed after it. So a commit stalls the whole log --
//! appends for the next epoch queue up behind it, and their arena slots stay
//! occupied until it finishes.
//!
//! That is the cost the design notes name, and it is the reason a real
//! consumer has to choose between the three strategies M14.3 implements. This
//! module deliberately picks the simplest one -- the drained flush -- because
//! it is the one whose correctness is easiest to see.
//!
//! # Reporting less than is true, on purpose
//!
//! Because the barrier is ring-wide, a commit of epoch *N* may in fact also
//! reach records already pushed into epoch *N+1*. Those records really are
//! durable when the flush completes. This module does **not** report them as
//! such: `durable_through` advances only to *N*, because *N* is what the
//! caller was promised and what the identity of that flush means. Reporting
//! less than reality is always safe; reporting more never is.

use std::collections::HashMap;
use std::io;
use std::os::windows::io::RawHandle;

use windows_ioring_sys::{Batch, Completion, FlushCoverage, FlushMode, IoRing};

/// The unit of durability. Records belong to exactly one, decided when the
/// append is accepted and never changed afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Epoch(pub u64);

/// Tracks which epoch is open, which commits are in flight, and how far
/// durability has actually reached.
pub struct Committer {
    open: u64,
    durable_through: Option<u64>,
    /// Flush `UserData` -> the epoch that flush closes. This map *is* the
    /// "carrying N as its identity" step: the ring hands back a `UserData` for
    /// the push, and that value names the epoch until its completion arrives.
    in_flight: HashMap<usize, u64>,
}

impl Committer {
    pub fn new() -> Self {
        Self {
            open: 0,
            durable_through: None,
            in_flight: HashMap::new(),
        }
    }

    /// The epoch records appended right now will join.
    pub fn open_epoch(&self) -> Epoch {
        Epoch(self.open)
    }

    /// The highest epoch known to be durable, or `None` if nothing is yet.
    ///
    /// This is the truthful answer the item asks for: it advances only when a
    /// commit's completion has actually been observed, never when one is
    /// pushed.
    pub fn durable_through(&self) -> Option<Epoch> {
        self.durable_through.map(Epoch)
    }

    /// Whether `epoch` is durable, per [`Committer::durable_through`].
    ///
    /// Monotonic by construction: a `true` here for epoch *N* implies `true`
    /// for every earlier epoch, which is the guarantee that lets a caller
    /// remember one number instead of a set.
    pub fn is_durable(&self, epoch: Epoch) -> bool {
        self.durable_through
            .is_some_and(|through| through >= epoch.0)
    }

    /// How many commits are pushed but not yet observed complete.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Close the open epoch and push its commit, returning the epoch that was
    /// closed. The next append joins the epoch after it.
    ///
    /// The flush is covering and uses a syncing mode, which together are the
    /// entire difference between a commit and a no-op that reports success:
    /// [`FlushCoverage::Unordered`] would not cover the epoch's writes, and
    /// [`FlushMode::NoSync`] would not reach the device.
    ///
    /// # Errors
    ///
    /// Any error from the flush push or the submit.
    pub fn commit(&mut self, ring: &mut IoRing, file: RawHandle) -> io::Result<Epoch> {
        let closing = self.open;
        let mut batch = Batch::new(ring);
        // SAFETY: `file` is the log's own handle and outlives every operation
        // pushed here; the log drains to empty before it closes.
        let user_data = unsafe {
            batch.flush_raw(
                file,
                FlushCoverage::CoversPrecedingOperations,
                FlushMode::Default,
            )
        }?;
        batch.submit()?;

        self.in_flight.insert(user_data, closing);
        self.open += 1;
        Ok(Epoch(closing))
    }

    /// Account for one popped completion that belongs to a commit.
    ///
    /// Returns the epoch that just became durable, or `None` if `completion`
    /// was not one of ours.
    ///
    /// # Errors
    ///
    /// The flush's own failure, if it failed. A failed commit advances
    /// nothing: the epoch it was closing is *not* durable, and saying so is
    /// the whole point of checking.
    pub fn claim(&mut self, completion: &Completion) -> io::Result<Option<Epoch>> {
        let Some(epoch) = self.in_flight.remove(&completion.user_data()) else {
            return Ok(None);
        };
        // Checked before anything advances. A commit that failed leaves
        // `durable_through` exactly where it was, so a caller asking about
        // that epoch keeps getting `false` -- which is the truthful answer.
        completion.result()?;

        // Commits are barrier-ordered against each other (D-24 holds an
        // operation pushed after a drained one until it completes), so
        // completions should arrive in epoch order. `max` rather than plain
        // assignment anyway: if that expectation is ever wrong, the reported
        // answer stays correct and only the assertion is noisy.
        debug_assert!(
            self.durable_through.is_none_or(|through| through < epoch),
            "commit completions should arrive in epoch order"
        );
        self.durable_through = Some(self.durable_through.map_or(epoch, |t| t.max(epoch)));
        Ok(Some(Epoch(epoch)))
    }
}
