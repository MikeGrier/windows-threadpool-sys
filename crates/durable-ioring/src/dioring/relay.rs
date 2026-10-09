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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::{Event, ResetMode};
use windows_ioring_sys::{
    Completion, EventDelivery, FlushCoverage, FlushMode, RegisteredFile, RegisteredSpan,
    WriteCaching as RingCaching,
};

use super::durability::{
    DEFAULT_LINEAGE, Due, Durability, Event as Synthesized, Flush, ProviderCall, WriteEnd,
};
use super::{Sidecar, TimeBase};
use crate::contract::EpochId;
use crate::ids::{DioringIds, Home, InstanceId, Lineage, OpId};
use crate::provider::{Answer, DomainCall, DomainOutcome};
use crate::types::IoTargetKey;
use crate::types::{Entry, Epoch, FailureCause, OpCompletion, OpKind, Outcome};

/// An entry of a dioring instance's queue.
pub(crate) type DioringEntry<E, B, C> = Entry<DioringIds<E>, B, C>;

/// The ring, inside its delivery.
pub(crate) type Delivery<E, B, C> = EventDelivery<B, Sidecar<E, C>>;

/// What a flush, or a held operation, is pushed against: a file's registration, or its handle.
#[derive(Clone, Debug)]
pub(crate) enum FlushTarget {
    Registered(RegisteredFile),
    Shared(SharedHandle),
}

impl FlushTarget {
    /// What a push is made against, borrowed.
    pub(crate) fn via(&self) -> Via<'_> {
        match self {
            FlushTarget::Registered(index) => Via::Registered(index),
            FlushTarget::Shared(handle) => Via::Shared(handle),
        }
    }
}

/// What a push is made against, borrowed from a file's slot or a held operation's target, so a
/// push made at once clones nothing.
#[derive(Clone, Copy)]
pub(crate) enum Via<'a> {
    Registered(&'a RegisteredFile),
    Shared(&'a SharedHandle),
}

/// The push of an owned operation, made where its buffer's bound holds: `IoBuf` for a write,
/// `IoBufMut` for a read. The caching choice is a write's alone.
pub(crate) type OwnedPush<B, S> = for<'r, 'v> fn(
    &mut windows_ioring_sys::Batch<'r, B, S>,
    Via<'v>,
    B,
    u64,
    RingCaching,
    S,
) -> Result<
    windows_ioring_sys::OperationId,
    windows_ioring_sys::PushRefused<B, S>,
>;

/// What an operation transfers: an owned buffer, with the push it needs, or a span of the
/// registration.
pub(crate) enum OpData<B, S> {
    Owned(B, OwnedPush<B, S>),
    Span(RegisteredSpan),
}

/// One consumer operation, as a push describes it.
pub(crate) struct OpPush<E: EpochId + 'static, B, C> {
    pub(crate) id: OpId,
    pub(crate) kind: OpKind<DioringIds<E>>,
    pub(crate) file: IoTargetKey,
    pub(crate) offset: u64,
    pub(crate) data: OpData<B, Sidecar<E, C>>,
    pub(crate) caching: RingCaching,
    pub(crate) context: C,
}

/// An operation held for its gate, with what its push will be made against, kept until it is
/// released or never issued (DI-D-23).
pub(crate) struct HeldPush<E: EpochId + 'static, B, C> {
    pub(crate) op: OpPush<E, B, C>,
    pub(crate) target: FlushTarget,
    /// The epoch it waits for, as the consumer named it.
    pub(crate) gate: Epoch<DioringIds<E>>,
}

/// What ending an instance takes from its core, in the one locked step that closes it.
pub(crate) struct Shut<E: EpochId + 'static, B, C, S> {
    /// The operations held for their gates, in push order: the kernel never saw them.
    pub(crate) held: Vec<HeldPush<E, B, C>>,
    /// What the caller read under the same lock.
    pub(crate) snapshot: S,
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
    /// The operations held for their gates, by push order.
    pub(crate) held: BTreeMap<u64, HeldPush<E, B, C>>,
    /// Held operations whose gates are durable, in the order released, waiting to be pushed by
    /// the consumer's next call: only a call on the instance can reach the registration a
    /// registered-span operation is pushed with.
    pub(crate) ready: VecDeque<OpId>,
    /// Requests for the consumer's provider, in the order made, waiting for the consumer's next
    /// call: one made under this lock could not be answered inside the provider's call without
    /// waiting for itself (DI-D-44).
    pub(crate) calls: Vec<ProviderCall>,
    /// A submission failed, so pushed operations may still sit in the submission queue; the next
    /// submission issues them (the ring crate's D-5).
    pub(crate) unsubmitted: bool,
    /// The consumer operations the ring holds, by file: counted from the push to the record of
    /// its completion, as `remove_target` reports them (DI-D-45). A file with none has no entry.
    pub(crate) in_flight: HashMap<IoTargetKey, usize>,
    /// Provider completions made and not yet answered, dropped ones included: ending waits for
    /// this to reach zero (DI-D-25).
    unanswered: usize,
    /// The instance is ending: no callback may reach the ring again, and nothing recorded from now
    /// on is reported.
    closed: bool,
    /// Failures armed by the fault seam: the next flush of the file to complete reports the Win32
    /// error instead of what the kernel did.
    #[cfg(feature = "fault-injection")]
    pub(crate) injected: Vec<(IoTargetKey, u32)>,
}

/// The state dioring's delivery callback reaches.
pub(crate) struct Relay<E: EpochId + 'static, B, C, K> {
    core: Mutex<Core<E, B, C, K>>,
    /// Auto-reset; the consumer's `readiness()` is a duplicate of it.
    readiness: Event,
    ring: OnceLock<Weak<Delivery<E, B, C>>>,
    /// Notified, under the core's lock, when the last unanswered provider completion is answered.
    answered: Condvar,
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
                held: BTreeMap::new(),
                ready: VecDeque::new(),
                calls: Vec::new(),
                unsubmitted: false,
                in_flight: HashMap::new(),
                unanswered: 0,
                closed: false,
                #[cfg(feature = "fault-injection")]
                injected: Vec::new(),
            }),
            readiness: Event::new(ResetMode::Auto, false)?,
            ring: OnceLock::new(),
            answered: Condvar::new(),
        })
    }

    /// Let the callback reach the ring, once the delivery exists.
    pub(crate) fn attach(&self, ring: &Arc<Delivery<E, B, C>>) {
        let attached = self.ring.set(Arc::downgrade(ring));
        debug_assert!(attached.is_ok(), "the ring is attached once");
    }

    /// Close the core, in one locked step that also reads `snapshot` and takes every held
    /// operation: no callback reaches the ring after this returns, nothing recorded afterwards is
    /// reported, and requests for the provider not yet made never will be (DI-D-25, DI-D-45).
    /// `None` if the core was already closed.
    pub(crate) fn shut<S>(
        &self,
        snapshot: impl FnOnce(&Core<E, B, C, K>) -> S,
    ) -> Option<Shut<E, B, C, S>> {
        let mut core = self.lock();
        if core.closed {
            return None;
        }
        core.closed = true;
        let snapshot = snapshot(&core);
        let held = std::mem::take(&mut core.held).into_values().collect();
        core.ready.clear();
        core.calls.clear();
        core.unpushed.clear();
        Some(Shut { held, snapshot })
    }

    /// Wait, with the core's lock released, until every provider completion made has been
    /// answered or dropped (DI-D-25).
    pub(crate) fn wait_for_answers(&self) {
        let mut core = self.lock();
        while core.unanswered > 0 {
            core = self
                .answered
                .wait(core)
                .unwrap_or_else(PoisonError::into_inner);
        }
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
        // The instance has ended: what the completion held is dropped here, and nothing is reported
        // or signalled. No test reaches it with a completion still to record, because ending
        // returns only once the ring has run down, and nothing can observe the instance afterwards.
        if core.closed {
            return;
        }
        let due = match held {
            Some((
                buffer,
                Sidecar::Consumer {
                    id,
                    kind,
                    file,
                    context,
                    ..
                },
            )) => {
                match core.in_flight.get_mut(&file) {
                    Some(1) => {
                        core.in_flight.remove(&file);
                    }
                    Some(count) => *count -= 1,
                    None => core.durability.inconsistent(
                        "a completion for a file with nothing in flight",
                        format_args!("operation {id:?} on {file:?}"),
                    ),
                }
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
                        // A kernel completion never reports a failure without its code (DI-D-47),
                        // so no test reaches this: only a ring breaking its contract could.
                        None => {
                            core.durability.inconsistent(
                                "a write's failed completion carried no error code",
                                format_args!("operation {id:?} on {file:?}"),
                            );
                            Due::default()
                        }
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
        core.ready.extend(due.released);
        core.calls.extend(due.calls);
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
        core.ready.extend(due.released);
        core.calls.extend(due.calls);
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
                Synthesized::NeverIssued { op, abandoned } => {
                    let Some(held) = core.held.remove(&op.seq) else {
                        debug_assert!(false, "a held operation with nothing held for it");
                        continue;
                    };
                    core.ready.retain(|ready| *ready != op);
                    Entry::Op(OpCompletion {
                        id: op,
                        kind: held.op.kind,
                        outcome: Outcome::NeverIssued {
                            abandoned: Epoch::new(name(abandoned.lineage), abandoned.epoch),
                        },
                        buffer: match held.op.data {
                            OpData::Owned(buffer, _) => Some(buffer),
                            OpData::Span(_) => None,
                        },
                        context: held.op.context,
                    })
                }
            };
            self.append(core, entry);
        }
    }
}

/// Where the consumer's provider answers (DI-D-27, DI-D-44). It runs on whatever thread answers --
/// inside the provider's call included, which dioring never makes with this lock held -- so, like a
/// callback, it takes the core's lock and then the ring's.
impl<E, B, C, K> Answer for Relay<E, B, C, K>
where
    E: EpochId + Send + Sync + 'static,
    B: Send + 'static,
    C: Send + 'static,
    K: TimeBase,
{
    fn issued(&self) {
        self.lock().unanswered += 1;
    }

    fn answer(&self, call: &DomainCall, outcome: DomainOutcome) {
        let outcome = match outcome {
            DomainOutcome::Succeeded => Ok(()),
            DomainOutcome::Failed(error) => Err(FailureCause::Provider {
                domain: call.name.clone(),
                error: Arc::new(error),
            }),
            DomainOutcome::Abandoned => Err(FailureCause::ProviderAbandoned {
                domain: call.name.clone(),
            }),
        };
        let mut core = self.lock();
        // Counted down even once the instance has ended, which is what ending waits for.
        match core.unanswered.checked_sub(1) {
            Some(left) => core.unanswered = left,
            None => core.durability.inconsistent(
                "a provider answered a completion never made",
                format_args!(
                    "lineage {}, seal {}, domain {:?}",
                    call.lineage, call.seal, call.name
                ),
            ),
        }
        if core.unanswered == 0 {
            self.answered.notify_all();
        }
        if core.closed {
            return;
        }
        let due = core
            .durability
            .provided(call.lineage, call.seal, call.domain, outcome);
        self.act(&mut core, due);
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
