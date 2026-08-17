// Copyright (c) 2026 Mike Grier
//! Operation identity and the live-identity registry shared by the backends.
//!
//! An operation's `OVERLAPPED` address alone is not a durable name for it.
//! Reclaiming an operation returns that address to the allocator, which may hand
//! it to a later operation, so an address retained past its operation's
//! completion can silently name a different, live operation. Cancellation acts
//! purely on that address, so without more information a stale name would cancel
//! the wrong operation through an entirely safe API.
//!
//! [`OperationId`] therefore pairs the address with a process-wide monotonic
//! generation taken at submission, and each backend records its live identities
//! in an [`OperationRegistry`]. Cancellation consults the registry first: an
//! identity whose address is not live, or is live under a different generation,
//! is rejected instead of being passed to the kernel.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

use windows_sys::Win32::System::IO::OVERLAPPED;

/// Source of the process-wide generation sequence.
///
/// Generations start at 1, so 0 is never a generation any real submission was
/// given. The sequence is process-wide rather than per-backend so that an
/// identity minted by one backend object can never collide with one minted by
/// another; at one submission per nanosecond a `u64` still takes centuries to
/// wrap, so exhaustion is not a practical concern.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// An identity for an in-flight operation: the address of its `OVERLAPPED`
/// together with the generation stamped on it at submission.
///
/// The address must not be dereferenced or freed; the kernel owns the storage
/// until the completion is claimed. The generation is what makes the identity
/// durable: addresses are recycled when operations are reclaimed, but a given
/// (address, generation) pair names exactly one submission for the life of the
/// process. Retaining an identity past its operation's completion is therefore
/// harmless -- the backend will reject it rather than act on whatever operation
/// currently occupies that address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationId {
    overlapped: *mut OVERLAPPED,
    generation: u64,
}

// SAFETY: an identity is inert data -- an address and a number. It owns nothing,
// and neither this type nor any backend ever dereferences the address: the
// registry compares it, and cancellation passes it to `CancelIoEx` as an opaque
// token. Moving one between threads is therefore no different from moving the
// address as an integer.
//
// This matters because cancelling from a thread other than the submitting one is
// the central use of an identity -- a timeout elsewhere aborting an in-flight
// operation -- and the raw pointer would otherwise make the type `!Send` and put
// that pattern out of reach.
unsafe impl Send for OperationId {}
unsafe impl Sync for OperationId {}

impl OperationId {
    /// Mint a new identity for an operation being submitted.
    ///
    /// Takes the next generation from the process-wide sequence, so every call
    /// yields a distinct identity even when `overlapped` repeats an address used
    /// by an earlier, already-reclaimed operation. A backend calls this exactly
    /// once per submission, at the moment it hands the storage to the kernel.
    #[must_use]
    pub fn mint(overlapped: *mut OVERLAPPED) -> Self {
        Self {
            overlapped,
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Rebuild an identity from an address and a generation a backend already
    /// observed together.
    ///
    /// This is the backend-facing counterpart to [`OperationId::mint`]: it is
    /// how a backend reconstructs the identity of an operation it knows only by
    /// address, such as when the pool hands a completion callback an
    /// `OVERLAPPED` and the registry supplies the generation recorded for it.
    ///
    /// It cannot be used to defeat the staleness check. An identity built from a
    /// generation that is no longer the one registered for the address is
    /// rejected exactly like any other stale identity, so reconstructing an
    /// identity confers no access that observing it did not already confer.
    #[must_use]
    pub fn from_parts(overlapped: *mut OVERLAPPED, generation: u64) -> Self {
        Self {
            overlapped,
            generation,
        }
    }

    /// The `OVERLAPPED` pointer this identity refers to.
    #[must_use]
    pub fn as_ptr(self) -> *mut OVERLAPPED {
        self.overlapped
    }

    /// The generation stamped on this identity at submission.
    #[must_use]
    pub fn generation(self) -> u64 {
        self.generation
    }
}

/// Lock a mutex, recovering the guard even if a previous holder panicked.
///
/// A poisoned lock here only means some callback panicked. The registry it
/// protects is a plain map that the backends keep exact through guards that run
/// while unwinding, so refusing to proceed would strand outstanding operations
/// rather than protect anything.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The set of operations a backend has submitted and not yet reclaimed.
///
/// The registry is the single source of truth for both questions a backend must
/// answer: how many operations are outstanding (rundown), and whether a given
/// identity still names a live operation (cancellation). Keeping them in one
/// structure means they cannot disagree.
///
/// At most one live operation exists per address at any moment -- the address is
/// only reusable once the previous operation's storage has been freed -- so the
/// map is keyed by address and holds the generation of whichever submission owns
/// it now.
#[derive(Debug)]
pub struct OperationRegistry {
    live: Mutex<HashMap<usize, u64>>,
    drained: Condvar,
}

impl OperationRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            live: Mutex::new(HashMap::new()),
            drained: Condvar::new(),
        }
    }

    /// Record a newly submitted operation.
    ///
    /// # Panics
    ///
    /// Panics if the address is already live, which would mean the backend
    /// submitted two operations against one piece of storage.
    pub fn insert(&self, id: OperationId) {
        let mut live = lock(&self.live);
        match live.entry(id.as_ptr() as usize) {
            Entry::Occupied(_) => panic!(
                "windows-overlapped-io-sys: operation storage {:p} was submitted while an earlier \
                 operation on the same storage was still outstanding",
                id.as_ptr()
            ),
            Entry::Vacant(slot) => {
                slot.insert(id.generation());
            }
        }
    }

    /// Remove a reclaimed operation, waking any rundown once the last one clears.
    ///
    /// Returns the generation that was recorded for the address, if any.
    pub fn remove(&self, overlapped: *mut OVERLAPPED) -> Option<u64> {
        let mut live = lock(&self.live);
        let generation = live.remove(&(overlapped as usize));
        if live.is_empty() {
            self.drained.notify_all();
        }
        generation
    }

    /// Whether this exact identity -- address *and* generation -- is still live.
    ///
    /// A retained identity whose operation has completed returns `false` even if
    /// its address has since been reissued to another operation.
    #[must_use]
    pub fn is_live(&self, id: OperationId) -> bool {
        lock(&self.live).get(&(id.as_ptr() as usize)) == Some(&id.generation())
    }

    /// The generation currently recorded for an address, if it is live.
    ///
    /// A backend uses this to rebuild the full identity of an operation it knows
    /// only by address, as when a completion arrives carrying its `OVERLAPPED`.
    #[must_use]
    pub fn generation_of(&self, overlapped: *mut OVERLAPPED) -> Option<u64> {
        lock(&self.live).get(&(overlapped as usize)).copied()
    }

    /// The number of operations submitted and not yet reclaimed.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.live).len()
    }

    /// Whether no operation is outstanding.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Block until every outstanding operation has been reclaimed.
    ///
    /// The caller must have arranged for the outstanding operations to complete
    /// -- by cancelling them, or because they are destined to finish -- or this
    /// waits indefinitely. It is used by backends whose completions arrive on
    /// threads the owner does not drive.
    pub fn wait_until_empty(&self) {
        let mut live = lock(&self.live);
        while !live.is_empty() {
            live = self
                .drained
                .wait(live)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
