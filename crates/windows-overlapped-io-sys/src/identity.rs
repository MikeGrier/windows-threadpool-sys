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
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

use windows_sys::Win32::System::IO::OVERLAPPED;

/// Source of the process-wide generation sequence.
///
/// Generations start at 1, so 0 is never a generation any real submission was
/// given. The sequence is process-wide rather than per-backend so that an
/// identity minted by one backend object can never collide with one minted by
/// another.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Take the next generation from `sequence`, refusing to wrap.
///
/// Wrapping would restart the sequence and hand out generations already in use,
/// which is exactly the stale-identity aliasing generations exist to prevent --
/// so the invariant is enforced here rather than only asserted in prose.
/// Exhaustion is not reachable in practice: at one submission per nanosecond a
/// `u64` still takes centuries.
///
/// The counter is a parameter so this boundary can be tested; production code
/// always passes [`NEXT_GENERATION`].
///
/// # Panics
///
/// Panics once the sequence is exhausted. The counter is then pinned at its
/// exhausted value, so a caught panic cannot resume minting recycled
/// generations on a later call.
fn next_generation(sequence: &AtomicU64) -> u64 {
    let generation = sequence.fetch_add(1, Ordering::Relaxed);
    if generation == u64::MAX {
        // `fetch_add` has already wrapped the stored value to 0, so without this
        // a caught panic would let the next call hand out generation 0 and walk
        // the whole sequence again. Pin it at the exhausted value instead, so
        // every later call takes this same branch.
        sequence.store(u64::MAX, Ordering::Relaxed);
        panic!(
            "the operation-generation sequence is exhausted; continuing would reissue \
             generations already in use and reintroduce stale-identity aliasing"
        );
    }
    generation
}

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
    ///
    /// # Panics
    ///
    /// Panics if the process-wide generation sequence is exhausted, rather than
    /// wrapping and reissuing generations already in use. A `u64` takes
    /// centuries to exhaust at one submission per nanosecond, so this is a
    /// guard on the type's uniqueness invariant rather than a reachable case.
    #[must_use]
    pub fn mint(overlapped: *mut OVERLAPPED) -> Self {
        Self {
            overlapped,
            generation: next_generation(&NEXT_GENERATION),
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
///
/// # Invariant enforced by this type
///
/// **An address must never be registered while it is available for reuse.**
/// Equivalently: a backend must deregister an operation *before* anything can
/// free its storage, never after. [`OperationRegistry::insert`] panics when this
/// is violated, because the alternative is silent corruption -- two live
/// operations sharing one map entry would make [`OperationRegistry::is_live`]
/// answer for the wrong one and let a cancellation reach an unrelated operation,
/// which is the exact class of bug generations exist to prevent.
///
/// The invariant is easy to break in a completion callback, where the natural
/// place to deregister (after the callback returns) is *later* than the point at
/// which the callback may free the storage -- for example by taking ownership of
/// the operation and dropping it. Deregister on callback entry instead.
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
    /// Call this once per submission, before the operation can complete.
    ///
    /// # Panics
    ///
    /// Panics if `id`'s address is **already registered** -- that is, if a
    /// previous operation at the same storage address has not been removed with
    /// [`OperationRegistry::remove`] yet.
    ///
    /// This is always a defect in the completion backend, never in the code
    /// calling that backend, and it is deliberately a panic rather than a
    /// silently-ignored duplicate: two live operations sharing one entry would
    /// corrupt exactly the guarantee this registry provides, letting a
    /// cancellation reach an operation the caller never named.
    ///
    /// The two ways a backend causes it:
    ///
    /// - **Deregistering too late.** If a completed operation is removed only
    ///   *after* its storage is freed, the allocator can hand that address to a
    ///   concurrent submission while the stale entry is still present. Remove the
    ///   operation before anything can free it -- on completion-callback entry
    ///   rather than on exit, since a callback may take ownership of the
    ///   operation and drop it part-way through.
    /// - **Submitting one operation's storage twice**, so a second submission
    ///   reuses an `OVERLAPPED` that is still in flight.
    pub fn insert(&self, id: OperationId) {
        let mut live = lock(&self.live);
        match live.entry(id.as_ptr() as usize) {
            Entry::Occupied(occupied) => panic!(
                "windows-overlapped-io-sys: operation storage {address:p} was registered for \
                 generation {new} while generation {existing} was still registered at the same \
                 address. An address must never be registered while it is available for reuse. \
                 This is a defect in the completion backend: it either deregistered a completed \
                 operation after its storage was freed rather than before (leaving a window in \
                 which a concurrent submission can be handed the same address), or submitted one \
                 operation's storage twice while it was still in flight.",
                address = id.as_ptr(),
                new = id.generation(),
                existing = occupied.get(),
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
    ///
    /// This is a snapshot, so it must not be used to guard a native cancellation:
    /// the answer can be stale by the time the caller acts on it. Use
    /// [`OperationRegistry::cancel_if_live`] for that.
    #[must_use]
    pub fn is_live(&self, id: OperationId) -> bool {
        lock(&self.live).get(&(id.as_ptr() as usize)) == Some(&id.generation())
    }

    /// Run `cancel` only if `id` still names a live operation, holding the
    /// registry guard across both the check and the call.
    ///
    /// Checking liveness and then cancelling as two steps is a race, not merely
    /// an imprecision: between the two, the operation can complete and be
    /// reclaimed, and a concurrent submission can be handed the same storage
    /// address. The native cancel would then reach an unrelated live operation --
    /// exactly what the generation is meant to prevent. Holding the guard closes
    /// that window, because a submission cannot register a reused address until
    /// it is released.
    ///
    /// `cancel` should perform only the native cancellation. It must not call
    /// back into this registry, which would deadlock on the same non-reentrant
    /// lock.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::NotFound`] without invoking `cancel` if `id` no
    /// longer names a live operation, or whatever `cancel` returns.
    pub fn cancel_if_live<F>(&self, id: OperationId, cancel: F) -> io::Result<()>
    where
        F: FnOnce() -> io::Result<()>,
    {
        let live = lock(&self.live);
        if live.get(&(id.as_ptr() as usize)) != Some(&id.generation()) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the operation named by this identity is no longer outstanding",
            ));
        }
        // The guard is deliberately still held: releasing it here would reopen
        // the window this function exists to close.
        let result = cancel();
        drop(live);
        result
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
