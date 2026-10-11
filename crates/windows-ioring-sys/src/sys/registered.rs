// Copyright (c) Mike Grier
//! Registering a [`Responses`] for one ring (M35.1, [D-85](../../DESIGN-NOTES.md#d-85)).
//!
//! # One ring, any thread
//!
//! A responder registered here answers the seam calls of the one ring it was
//! registered for, on whatever thread makes them. That is what a ring used
//! from more than one thread needs -- a pool thread draining it, a consumer
//! submitting to it -- and what the thread-local form in `installed` cannot
//! give: a call from a thread with nothing installed reaches the kernel
//! silently, so an injected failure can be undone by a real call on another
//! thread and a test pass for the wrong reason.
//!
//! Tests still cannot answer each other's calls, which is what made the
//! thread-local form thread-local: two tests do not share a ring. The key is
//! the ring's `RingId`, which is process-unique and never reused, where a raw
//! handle value can be reused once `CloseIoRing` has run.
//!
//! # How long a registration lasts
//!
//! **Until its ring drops**, after the ring's rundown and before
//! `CloseIoRing` -- so a responder holding operations the kernel never saw
//! still answers the rundown that pops them -- or until
//! [`Registration::end`] ends it earlier. **Dropping the [`Registration`]
//! ends nothing.** That is deliberate: locals drop in reverse declaration
//! order, so a registration ended by its handle's drop would, in the ordinary
//! `let ring = ..; let registration = register(&ring, ..);` shape, end before
//! the ring's rundown ran -- the very ordering hazard this form exists to
//! remove.
//!
//! # Precedence, re-entrancy, and concurrency
//!
//! - A ring's responder is consulted before a thread's: it is the more
//!   specific of the two, and the one D-85 keeps.
//! - A responder is not consulted re-entrantly for its own ring. A call it
//!   makes into the seam for that ring finds it already answering on this
//!   thread and falls through to the next layer, as the thread-local form
//!   does.
//! - Calls on one ring from two threads are serialized on the responder's
//!   lock. A ring is not `Sync`, so they do not overlap in practice; a call
//!   that did would wait rather than fall through to the kernel, which would
//!   be wrong silently.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::Responses;
use crate::IoRing;
use crate::accounting::RingId;

/// A registered responder, behind its own lock so one ring's call never waits
/// on another ring's. `None` once the registration has ended, for a call that
/// cloned it just before.
type Slot = Arc<Mutex<Option<Box<dyn Responses + Send>>>>;

/// Every live registration. A `Vec` rather than a map: it holds a handful of
/// entries at most, and `Mutex::new(Vec::new())` is `const`.
static REGISTRY: Mutex<Vec<(RingId, Slot)>> = Mutex::new(Vec::new());

thread_local! {
    /// The rings whose responders this thread is answering for right now, so
    /// a re-entrant call is recognised rather than deadlocking on the slot.
    static ANSWERING: RefCell<Vec<RingId>> = const { RefCell::new(Vec::new()) };
}

/// Lock `mutex`. A responder that panicked while answering leaves its own
/// state as the panic found it; the registry itself is only ever pushed to and
/// removed from, so neither is left inconsistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Marks `ring` as being answered on this thread until dropped.
struct Answering(RingId);

impl Answering {
    /// `None` if this thread is already answering for `ring`, or its
    /// thread-locals are gone.
    fn enter(ring: RingId) -> Option<Self> {
        ANSWERING
            .try_with(|answering| {
                let mut answering = answering.try_borrow_mut().ok()?;
                if answering.contains(&ring) {
                    return None;
                }
                answering.push(ring);
                Some(Self(ring))
            })
            .ok()
            .flatten()
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        let _ = ANSWERING.try_with(|answering| {
            if let Ok(mut answering) = answering.try_borrow_mut()
                && let Some(at) = answering.iter().rposition(|ring| *ring == self.0)
            {
                answering.remove(at);
            }
        });
    }
}

/// Run `f` against `ring`'s responder, if it has one and this thread is not
/// already answering for it. `None` is the seam's signal to try the next
/// layer.
pub(crate) fn with<R>(ring: RingId, f: impl FnOnce(&mut dyn Responses) -> R) -> Option<R> {
    let slot = {
        let registry = lock(&REGISTRY);
        let (_, slot) = registry.iter().find(|(id, _)| *id == ring)?;
        Arc::clone(slot)
    };
    let _answering = Answering::enter(ring)?;
    let mut responder = lock(&slot);
    let responder = responder.as_mut()?;
    Some(f(&mut **responder))
}

/// Register `responder` for the ring identified by `ring`.
///
/// # Panics
///
/// If that ring already has one: which of two answered a call would depend on
/// registration order, as nesting would for the thread-local form.
pub(crate) fn register_id(ring: RingId, responder: Box<dyn Responses + Send>) -> Registration {
    let mut registry = lock(&REGISTRY);
    assert!(
        !registry.iter().any(|(id, _)| *id == ring),
        "a kernel responder is already registered for this ring"
    );
    registry.push((ring, Arc::new(Mutex::new(Some(responder)))));
    Registration { ring }
}

/// End `ring`'s registration, if it has one, handing its responder back.
///
/// The responder is taken under its own lock, so a call answering for the ring
/// on another thread finishes first, and a call that had already found the
/// slot finds it empty and falls through.
fn end_id(ring: RingId) -> Option<Box<dyn Responses + Send>> {
    let slot = {
        let mut registry = lock(&REGISTRY);
        let at = registry.iter().position(|(id, _)| *id == ring)?;
        registry.swap_remove(at).1
    };
    lock(&slot).take()
}

/// `ring` is dropping, and its rundown has run: end its registration, if it
/// has one. Called by `IoRing`'s `Drop` before `CloseIoRing`.
pub(crate) fn ring_dropped(ring: RingId) {
    // Dropped here, outside every lock.
    drop(end_id(ring));
}

/// Register `responder` to answer `ring`'s seam calls, on whatever thread
/// makes them, until the ring drops or [`Registration::end`] is called.
///
/// See the module documentation for why dropping the returned
/// [`Registration`] does **not** end it.
///
/// # Panics
///
/// If `ring` already has a responder registered.
pub fn register<T, X>(ring: &IoRing<T, X>, responder: Box<dyn Responses + Send>) -> Registration {
    register_id(ring.ring_id(), responder)
}

/// A responder's registration for one ring (D-85).
///
/// Ends when its ring drops, or at [`Registration::end`]. Dropping this value
/// ends nothing; see the module documentation for why.
#[derive(Debug)]
pub struct Registration {
    ring: RingId,
}

impl Registration {
    /// End the registration now, before its ring drops, handing the responder
    /// back. Once it returns, the ring's calls reach whatever answers next --
    /// a thread's responder, or the kernel.
    ///
    /// `None` if the registration had already ended because its ring dropped.
    pub fn end(self) -> Option<Box<dyn Responses + Send>> {
        end_id(self.ring)
    }
}

#[cfg(test)]
mod tests;
