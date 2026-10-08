// Copyright (c) 2026 Mike Grier
//! dioring's core, under dioring's own lock, and the readiness event: what the consumer's calls and
//! `EventDelivery`'s callback both reach (DI-D-18, DI-D-28).
//!
//! The callback runs on pool threads with the ring's lock released, and two invocations may
//! overlap (`windows-ioring-sys`' D-83). It binds to exactly that: each completion is recorded
//! under dioring's lock, so the queue's order is the order completions were recorded in, whichever
//! thread recorded them, and the durability state changes in that same order. Nothing here assumes
//! an order between two callbacks.
//!
//! **Lock order: dioring's, then the ring's** (DI-2.3 point 10). A consumer push holds the core
//! while it pushes, so its write is recorded before its completion can be; a callback that makes a
//! flush due pushes it while holding the core, the D-83 contract being what makes that safe.
//!
//! **Reaching the ring from the callback.** The callback holds the relay, and the relay holds only a
//! `Weak` to the delivery: the delivery owns the callback, so a strong reference would be a cycle.
//! The `Weak` is upgraded only while the core is locked, and released before it is unlocked, and an
//! ending instance marks the core closed under that same lock. So once the instance has closed the
//! core, no callback holds the delivery, and the delivery is dropped on the instance's own thread --
//! never inside one of its callbacks, where dropping it would wait for itself.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::{Event, ResetMode};
use windows_ioring_sys::{Completion, EventDelivery, FlushCoverage, FlushMode, RegisteredFile};

use super::durability::{DEFAULT_LINEAGE, Due, Durability, Event as Synthesized, Flush, WriteEnd};
use super::{Sidecar, TimeBase};
use crate::contract::EpochId;
use crate::ids::{DioringIds, Home, InstanceId, Lineage};
#[cfg(feature = "fault-injection")]
use crate::types::FileKey;
use crate::types::{Entry, Epoch, OpCompletion, OpKind, Outcome};

/// An entry of a dioring instance's queue.
pub(crate) type DioringEntry<E, B, C> = Entry<DioringIds<E>, B, C>;

/// The ring, inside its delivery.
pub(crate) type Delivery<E, B, C> = EventDelivery<B, Sidecar<E, C>>;

/// What a flush is pushed against: a file's registration, or its handle.
#[derive(Clone, Debug)]
pub(crate) enum FlushTarget {
    Registered(RegisteredFile),
    Shared(SharedHandle),
}

/// The state under dioring's lock.
pub(crate) struct Core<E: EpochId + 'static, B, C, K> {
    queue: VecDeque<DioringEntry<E, B, C>>,
    /// The durability of every lineage.
    pub(crate) durability: Durability<E, FlushTarget, K>,
    /// The instance, for the lineages its entries and flushes name.
    instance: InstanceId,
    /// Flushes due that the ring refused, pushed again at the next chance.
    unpushed: Vec<Flush<E, FlushTarget>>,
    /// A submission failed, so pushed operations may still sit in the submission queue; the next
    /// submission issues them (the ring crate's D-5).
    pub(crate) unsubmitted: bool,
    /// The instance is ending: no callback may reach the ring again.
    closed: bool,
    /// Failures armed by the fault seam: the next flush of the file to complete reports the Win32
    /// error instead of what the kernel did.
    #[cfg(feature = "fault-injection")]
    pub(crate) injected: Vec<(FileKey, u32)>,
}

/// The state dioring's delivery callback reaches.
pub(crate) struct Relay<E: EpochId + 'static, B, C, K> {
    core: Mutex<Core<E, B, C, K>>,
    /// Auto-reset; the consumer's `readiness()` is a duplicate of it.
    readiness: Event,
    ring: OnceLock<Weak<Delivery<E, B, C>>>,
}

impl<E: EpochId + 'static, B, C, K> Relay<E, B, C, K> {
    /// An empty queue, nothing sealed, and an unsignalled readiness event; failures are stamped
    /// with `clock`.
    pub(crate) fn new(instance: InstanceId, clock: K) -> io::Result<Self> {
        Ok(Self {
            core: Mutex::new(Core {
                queue: VecDeque::new(),
                durability: Durability::new(instance, clock),
                instance,
                unpushed: Vec::new(),
                unsubmitted: false,
                closed: false,
                #[cfg(feature = "fault-injection")]
                injected: Vec::new(),
            }),
            readiness: Event::new(ResetMode::Auto, false)?,
            ring: OnceLock::new(),
        })
    }

    /// Let the callback reach the ring, once the delivery exists.
    pub(crate) fn attach(&self, ring: &Arc<Delivery<E, B, C>>) {
        let attached = self.ring.set(Arc::downgrade(ring));
        debug_assert!(attached.is_ok(), "the ring is attached once");
    }

    /// No callback reaches the ring after this returns.
    pub(crate) fn close(&self) {
        self.lock().closed = true;
    }

    /// A duplicate of the readiness event.
    pub(crate) fn readiness(&self) -> io::Result<Event> {
        self.readiness.try_clone()
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Core<E, B, C, K>> {
        // A panic while the core was held leaves it consistent: each update is completed before
        // anything that can panic runs.
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next entry, if any.
    pub(crate) fn pop(core: &mut Core<E, B, C, K>) -> Option<DioringEntry<E, B, C>> {
        core.queue.pop_front()
    }

    /// Append an entry, and set the readiness event when the queue was empty: the entry is
    /// poppable before the event is set, and every empty-to-non-empty transition sets it
    /// (DI-D-28). A transition the consumer races -- popping the entry before the set lands --
    /// leaves it a wake with nothing to pop, which the contract calls normal.
    fn append(&self, core: &mut Core<E, B, C, K>, entry: DioringEntry<E, B, C>) {
        let was_empty = core.queue.is_empty();
        core.queue.push_back(entry);
        if was_empty {
            // Setting an event this instance created, with full access, does not fail.
            let set = self.readiness.set();
            debug_assert!(set.is_ok(), "setting the readiness event failed: {set:?}");
        }
    }
}

impl<E, B, C, K> Relay<E, B, C, K>
where
    E: EpochId + Send + Sync + 'static,
    B: Send + 'static,
    C: Send + 'static,
    K: TimeBase,
{
    /// Record one ring completion. Called from `EventDelivery`'s callback, on a pool thread.
    ///
    /// Panicking here would abort the process, so nothing in this path panics on a value it
    /// receives. A completion with no sidecar cannot reach it -- dioring pushes nothing outside the
    /// inventory -- and is dropped, with a debug assertion.
    pub(crate) fn record(&self, completion: Completion, held: Option<(Option<B>, Sidecar<E, C>)>) {
        let mut core = self.lock();
        let due = match held {
            Some((
                buffer,
                Sidecar::Consumer {
                    id, kind, context, ..
                },
            )) => {
                let outcome = match completion.result() {
                    // A transfer is never longer than the `u32` length it was pushed with.
                    Ok(transferred) => {
                        Outcome::Transferred(u32::try_from(transferred).unwrap_or(u32::MAX))
                    }
                    Err(error) => Outcome::Failed(error),
                };
                let end = WriteEnd::of(&outcome);
                // The completion goes on the queue before anything it makes due, so a `Durable` it
                // leads to follows it (guarantee 5).
                self.append(
                    &mut core,
                    Entry::Op(OpCompletion {
                        id,
                        kind,
                        outcome,
                        buffer,
                        context,
                    }),
                );
                match kind {
                    OpKind::Write { .. } => match end {
                        Some(end) => core.durability.completed(id, end),
                        None => Due::default(),
                    },
                    OpKind::Read => Due::default(),
                }
            }
            Some((_, Sidecar::Commit { through, file })) => {
                #[cfg(feature = "fault-injection")]
                let completion = match core.injected.iter().position(|(f, _)| *f == file) {
                    Some(index) => {
                        let (_, code) = core.injected.remove(index);
                        completion
                            .with_injected_failure(windows_ioring_sys::InjectedFailure::Win32(code))
                    }
                    None => completion,
                };
                let result = completion.result().map(|_| ());
                if through.lineage.instance == core.instance {
                    core.durability
                        .flushed(through.lineage.seq, through.id, file, result)
                } else {
                    debug_assert!(false, "a flush for another instance's lineage");
                    Due::default()
                }
            }
            None => {
                debug_assert!(false, "a completion for an operation dioring did not push");
                Due::default()
            }
        };
        self.act(&mut core, due);
    }

    /// Act on what a change made off the consumer's call path -- in a callback, or in a handle's
    /// release -- unless the instance is ending.
    fn act(&self, core: &mut Core<E, B, C, K>, due: Due<E, FlushTarget>) {
        if core.closed {
            return;
        }
        let ring = self.ring.get().and_then(Weak::upgrade);
        match ring {
            Some(ring) => {
                // A failed submission stays flagged for the consumer's next push or pop to retry
                // and report.
                let _ = self.apply(core, due, &ring);
            }
            None => self.apply_without_ring(core, due),
        }
        // `ring` is dropped here, before the core is unlocked.
    }

    /// Act on what a change made due: append its entries, in order, and push every flush now due,
    /// with any the ring refused before, then submit -- which also retries a submission that failed
    /// earlier. Returns the submission's error, if it failed: the operations stay queued, and the
    /// next submission issues them.
    pub(crate) fn apply(
        &self,
        core: &mut Core<E, B, C, K>,
        due: Due<E, FlushTarget>,
        ring: &Delivery<E, B, C>,
    ) -> Option<io::Error> {
        self.append_events(core, due.events);
        let mut flushes = std::mem::take(&mut core.unpushed);
        flushes.extend(due.flushes);
        if flushes.is_empty() && !core.unsubmitted {
            return None;
        }
        let instance = core.instance;
        let mut scope = ring.scope();
        let mut batch = scope.batch();
        for flush in flushes {
            let sidecar = Sidecar::Commit {
                through: Epoch::new(
                    Lineage {
                        instance,
                        seq: flush.lineage,
                    },
                    flush.through,
                ),
                file: flush.file,
            };
            let pushed = match &flush.target {
                FlushTarget::Registered(index) => {
                    batch.flush_owned(index, sidecar, FlushCoverage::Unordered, FlushMode::Default)
                }
                FlushTarget::Shared(handle) => batch.flush_owned(
                    handle,
                    sidecar,
                    FlushCoverage::Unordered,
                    FlushMode::Default,
                ),
            };
            if pushed.is_err() {
                // No test reaches a refused flush: the submission queue has room for it, because
                // every push is submitted at once. A refusal leaves the seal waiting for the retry.
                core.unpushed.push(flush);
            }
        }
        let submitted = batch.submit().err();
        core.unsubmitted = submitted.is_some();
        submitted
    }

    /// As `apply`, before the ring is attached or once it is gone: flushes wait for a later chance.
    fn apply_without_ring(&self, core: &mut Core<E, B, C, K>, due: Due<E, FlushTarget>) {
        self.append_events(core, due.events);
        core.unpushed.extend(due.flushes);
    }

    fn append_events(&self, core: &mut Core<E, B, C, K>, events: Vec<Synthesized<E>>) {
        let instance = core.instance;
        let name = |seq| Lineage { instance, seq };
        for event in events {
            let entry = match event {
                Synthesized::Durable { lineage, through } => Entry::Durable {
                    through: Epoch::new(name(lineage), through),
                },
                Synthesized::Blocked {
                    lineage,
                    through,
                    by,
                } => Entry::Blocked {
                    through: Epoch::new(name(lineage), through),
                    by,
                },
                Synthesized::Failed(failed) => Entry::Failed(failed),
                Synthesized::Abandoned {
                    failure,
                    suspect,
                    markings,
                } => Entry::Abandoned {
                    failure,
                    suspect,
                    markings,
                },
                Synthesized::Healed {
                    failure,
                    suspect,
                    markings,
                } => Entry::Healed {
                    failure,
                    suspect,
                    markings,
                },
                Synthesized::Marked { failure, marking } => Entry::Marked { failure, marking },
                Synthesized::LineageEnded {
                    lineage,
                    abandoned_through,
                } => Entry::LineageEnded {
                    lineage: name(lineage),
                    abandoned_through,
                },
            };
            self.append(core, entry);
        }
    }
}

/// Where a lineage's handles report the release of the last of them (DI-D-40). It runs on whatever
/// thread dropped that handle, so, like a callback, it takes the core's lock and then the ring's.
impl<E, B, C, K> Home for Relay<E, B, C, K>
where
    E: EpochId + Send + Sync + 'static,
    B: Send + 'static,
    C: Send + 'static,
    K: TimeBase,
{
    fn release(&self, lineage: Lineage) {
        let mut core = self.lock();
        // A lineage already ended or retired, or another instance's, has nothing left to end; the
        // default's last handle is the instance's own, released as the instance ends.
        if core.closed
            || lineage.instance != core.instance
            || lineage.seq == DEFAULT_LINEAGE
            || !core.durability.is_live(lineage.seq)
        {
            return;
        }
        let due = core.durability.end(lineage.seq);
        self.act(&mut core, due);
    }
}
