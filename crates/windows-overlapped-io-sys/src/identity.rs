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

/// Take the next generation from `sequence`, or `None` once it is exhausted.
///
/// This is the whole mechanism; [`next_generation`] only adds the panic. Having
/// a non-panicking form is not merely tidy: the concurrent exhaustion test hits
/// the boundary tens of thousands of times across several threads, and doing
/// that through the panicking form would mean either a flood of panic output or
/// worker threads swapping the *process-global* panic hook, which would race
/// each other and silence diagnostics for every other test in the binary.
///
/// The counter is a parameter so the boundary can be tested; production code
/// always passes [`NEXT_GENERATION`].
fn try_next_generation(sequence: &AtomicU64) -> Option<u64> {
    // A single atomic update, not an increment followed by a repair. `fetch_add`
    // would wrap the stored value to zero and leave it there until a separate
    // `store` could pin it -- and a thread arriving in that window would take 0,
    // then 1, 2, ... and mint successfully, which is exactly the recycled
    // generation this refuses to produce. Saturating inside the update means the
    // counter never transiently holds a wrapped value, so there is no window.
    sequence
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            // `then`, not `then_some`: the latter is eager, so `current + 1`
            // would overflow at the boundary before the guard could apply.
            (current != u64::MAX).then(|| current + 1)
        })
        .ok()
}

/// Take the next generation from `sequence`, refusing to wrap.
///
/// Wrapping would restart the sequence and hand out generations already in use,
/// which is exactly the stale-identity aliasing generations exist to prevent --
/// so the invariant is enforced here rather than only asserted in prose.
/// Exhaustion is not reachable in practice: at one submission per nanosecond a
/// `u64` still takes centuries.
///
/// # Panics
///
/// Panics once the sequence is exhausted, and every later call panics too: the
/// counter saturates at `u64::MAX` rather than passing it, so a caught panic
/// cannot resume minting recycled generations.
fn next_generation(sequence: &AtomicU64) -> u64 {
    try_next_generation(sequence).unwrap_or_else(|| {
        panic!(
            "the operation-generation sequence is exhausted; continuing would reissue \
             generations already in use and reintroduce stale-identity aliasing"
        )
    })
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

    /// Assemble an identity from an address and a generation chosen by the
    /// caller, without checking that they belong together.
    ///
    /// Backends do not need this: [`OperationRegistry::remove`] and
    /// [`OperationRegistry::identify`] hand back a whole `OperationId`,
    /// assembled from the pair the registry itself recorded, so the normal path
    /// from a completion to its identity never supplies a generation.
    ///
    /// This exists for tests that must synthesize an identity the registry never
    /// issued -- a stale one, or one from a generation ahead of the current --
    /// in order to prove such an identity is rejected.
    ///
    /// # Safety
    ///
    /// The caller must have observed `overlapped` and `generation` together as
    /// one operation's identity, or must be deliberately forging an identity in
    /// order to assert that it is refused.
    ///
    /// Forging is not memory-unsafe -- cancelling a live operation is
    /// well-defined and no storage can be reclaimed twice by it -- but it defeats
    /// the isolation the generation exists to provide. A caller holding `(p, g)`
    /// could otherwise construct `(p, g + 1)` and, if the next submission reusing
    /// `p` were stamped with that generation, cancel an operation it never
    /// submitted. That is why this is not a safe constructor:
    ///
    /// ```compile_fail
    /// # use windows_overlapped_io_sys::OperationId;
    /// fn forge_the_next_one(observed: OperationId) -> OperationId {
    ///     OperationId::forge(observed.as_ptr(), observed.generation() + 1)
    /// }
    /// ```
    ///
    /// The same call compiles once the caller takes on the obligation, so what
    /// the example above rejects is the missing `unsafe` rather than anything
    /// else about the code:
    ///
    /// ```
    /// # use windows_overlapped_io_sys::OperationId;
    /// fn rebuild(observed: OperationId) -> OperationId {
    ///     // SAFETY: both halves came from one identity, so they were observed
    ///     // together by construction.
    ///     unsafe { OperationId::forge(observed.as_ptr(), observed.generation()) }
    /// }
    /// ```
    #[must_use]
    pub unsafe fn forge(overlapped: *mut OVERLAPPED, generation: u64) -> Self {
        Self {
            overlapped,
            generation,
        }
    }

    /// Assemble an identity the registry has just looked up, in-crate.
    pub(crate) fn from_recorded_parts(overlapped: *mut OVERLAPPED, generation: u64) -> Self {
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
    /// Returns the identity that was registered for the address, if any --
    /// assembled here from the pair this registry recorded, so a backend never
    /// has to supply a generation and cannot get the pairing wrong.
    pub fn remove(&self, overlapped: *mut OVERLAPPED) -> Option<OperationId> {
        let mut live = lock(&self.live);
        let generation = live.remove(&(overlapped as usize));
        if live.is_empty() {
            self.drained.notify_all();
        }
        generation.map(|generation| OperationId::from_recorded_parts(overlapped, generation))
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

    /// The identity currently registered for an address, if it is live.
    ///
    /// A backend uses this to recover the full identity of an operation it knows
    /// only by address, as when a completion arrives carrying its `OVERLAPPED`.
    /// The generation comes from this registry rather than from the caller, so
    /// the returned identity always names the operation that is actually live at
    /// that address -- there is no pairing for a caller to get wrong, or to
    /// choose.
    #[must_use]
    pub fn identify(&self, overlapped: *mut OVERLAPPED) -> Option<OperationId> {
        lock(&self.live)
            .get(&(overlapped as usize))
            .copied()
            .map(|generation| OperationId::from_recorded_parts(overlapped, generation))
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
