// Copyright (c) 2026 Mike Grier
//! The Model A front end ([DI-D-31](../DESIGN-NOTES.md#di-d-31),
//! [DI-D-32](../DESIGN-NOTES.md#di-d-32)): an instance whose entries are handed, one at a time and
//! in queue order, to a handler on thread-pool threads, and whose operations are reachable from any
//! thread through `&self`.
//!
//! Written once over the trait, so it serves every implementation whose readiness signal is an
//! `Event` (DI-D-52 Q6). It waits on that signal through a thread-pool wait, and on each wake pops
//! to `None` before waiting again (DI-D-28). The handler is called with the instance's lock
//! released, so it may call back in through the [`DeliveryHandle`] it is given; it runs under a
//! lock of its own, which only the single deliverer takes.

use std::fmt;
use std::io;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use win_sync_sys::Event;
use windows_ioring_sys::{IoBuf, IoBufMut, RegisteredSpan};
use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitActivation};

use durable_ioring::{
    AddTargetError, DurabilityRequest, EndLineageError, Epoch, EpochState, FailureInfo,
    ImportScope, Imported, IoTargetKey, Leftovers, LineageInfo, ReadOptions, RemoveTargetError,
    Resolution, ResolveError, RetireLineageError, SealError, Tag, UnknownLineage, WriteOptions,
};
use durable_ioring::{
    DurableRing, EntryOf, EpochIdOf, FailureIdOf, HandleOf, Lin, PushResult, RegisteredBufferRing,
    TokenOf,
};
use durable_ioring::{ReadKey, TargetKey, WriteKey};

#[cfg(test)]
mod tests;

/// What the deliverer hands its handler: an entry, or why it could not pop one.
///
/// A failure is the instance's own pop failing -- a submission its doorbell could not issue, or a
/// ring it could not read ([DI-D-58](../../durable-ioring/DESIGN-NOTES.md#di-d-58)). It is reported
/// once per wake, and the delivery goes on: the next wake delivers what can be, and a cause that
/// stays is reported again. The handler's remedy for one that stays is to stop and drop the
/// delivery, which hands the instance back (into_inner) or closes it (close).
#[derive(Debug)]
pub enum DeliveryEvent<E> {
    /// The next entry, in queue order.
    Entry(E),
    /// The instance's `pop` failed. The error says why, and downcasts to the cause the instance
    /// reported, as an instance's own `pop` error does.
    Failure(io::Error),
}

/// The handler, as the deliverer holds it.
type Handler<D> = dyn FnMut(DeliveryEvent<EntryOf<D>>, &DeliveryHandle<D>) + Send;

/// What the deliverer and the owner share. The handler is last, so a front end built with one
/// handler type can be held as the trait object.
struct Inner<D, H: ?Sized> {
    handle: DeliveryHandle<D>,
    handler: Mutex<H>,
}

/// The operations of an instance whose delivery an [`EntryDelivery`] owns, through `&self` from
/// any thread. The handler is given one; the owner derefs to its own, so both offer the same
/// operations from one definition.
///
/// Each call takes the front end's lock on the instance for its duration -- dioring's lock in
/// DI-D-18's order, taken before the ring's -- and the handler is called with that lock released.
///
/// # What it withholds
///
/// - **`pop` and `readiness`**: the delivery is the single deliverer and the signal's one waiter
///   (DI-D-28). A second popper would take entries out of order; a second waiter would steal wakes.
/// - **Any `&mut` to the instance**, including through a closure. Safe code could then replace the
///   instance while the wait stayed armed on the old one's signal, and delivery would stop without
///   an error -- the ring crate's measured defect
///   ([D-43](../../windows-ioring-sys/DESIGN-NOTES.md#d-43)), which a closure does not close.
///   Registered bytes are therefore reached through closures over the bytes alone.
///
/// Not constructible, and not `Clone`: a handler holds one only for the length of a call, so it
/// cannot keep the front end alive.
pub struct DeliveryHandle<D> {
    /// `None` only after [`EntryDelivery::into_inner`] has quiesced the delivery and taken it.
    ring: Mutex<Option<D>>,
}

impl<D> DeliveryHandle<D> {
    /// Run `f` on the instance under the front end's lock.
    fn with<R>(&self, f: impl FnOnce(&mut D) -> R) -> R {
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        f(ring
            .as_mut()
            .expect("the instance is taken only once nothing can reach this handle"))
    }

    fn take(&self) -> D {
        self.ring
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("the instance is taken once")
    }
}

impl<D: DurableRing> DeliveryHandle<D> {
    /// [`DurableRing::add_target`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::add_target`].
    pub fn add_target<K: TargetKey>(
        &self,
        key: K,
        target: D::Target,
    ) -> Result<(), AddTargetError<D::Target>> {
        self.with(|ring| ring.add_target(key, target))
    }

    /// [`DurableRing::remove_target`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::remove_target`].
    pub fn remove_target(&self, key: IoTargetKey) -> Result<D::Target, RemoveTargetError<D::Ids>> {
        self.with(|ring| ring.remove_target(key))
    }

    /// [`DurableRing::default_lineage`].
    pub fn default_lineage(&self) -> HandleOf<D> {
        self.with(|ring| ring.default_lineage())
    }

    /// [`DurableRing::mint_lineage`].
    pub fn mint_lineage(&self, description: Option<String>) -> HandleOf<D> {
        self.with(|ring| ring.mint_lineage(description))
    }

    /// [`DurableRing::lineages`].
    pub fn lineages(&self) -> Vec<LineageInfo<D::Ids>> {
        self.with(|ring| ring.lineages())
    }

    /// [`DurableRing::end_lineage`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::end_lineage`].
    pub fn end_lineage(&self, handle: HandleOf<D>) -> Result<(), EndLineageError<D::Ids>> {
        self.with(|ring| ring.end_lineage(handle))
    }

    /// [`DurableRing::retire_lineage`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::retire_lineage`].
    pub fn retire_lineage(&self, handle: HandleOf<D>) -> Result<(), RetireLineageError<D::Ids>> {
        self.with(|ring| ring.retire_lineage(handle))
    }

    /// [`DurableRing::write`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::write`].
    pub fn write<K: WriteKey>(
        &self,
        file: K,
        offset: u64,
        buffer: D::Buffer,
        tag: Tag<'_, D::Ids>,
        context: D::Context,
    ) -> PushResult<D>
    where
        D::Buffer: IoBuf,
    {
        self.with(|ring| ring.write(file, offset, buffer, tag, context))
    }

    /// [`DurableRing::write_with`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::write_with`].
    pub fn write_with<K: WriteKey>(
        &self,
        file: K,
        offset: u64,
        buffer: D::Buffer,
        tag: Tag<'_, D::Ids>,
        context: D::Context,
        options: WriteOptions<D::Ids>,
    ) -> PushResult<D>
    where
        D::Buffer: IoBuf,
    {
        self.with(|ring| ring.write_with(file, offset, buffer, tag, context, options))
    }

    /// [`DurableRing::read`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::read`].
    pub fn read<K: ReadKey>(
        &self,
        file: K,
        offset: u64,
        buffer: D::Buffer,
        context: D::Context,
    ) -> PushResult<D>
    where
        D::Buffer: IoBufMut,
    {
        self.with(|ring| ring.read(file, offset, buffer, context))
    }

    /// [`DurableRing::read_with`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::read_with`].
    pub fn read_with<K: ReadKey>(
        &self,
        file: K,
        offset: u64,
        buffer: D::Buffer,
        context: D::Context,
        options: ReadOptions<D::Ids>,
    ) -> PushResult<D>
    where
        D::Buffer: IoBufMut,
    {
        self.with(|ring| ring.read_with(file, offset, buffer, context, options))
    }

    /// [`DurableRing::make_durable_through`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::make_durable_through`].
    pub fn make_durable_through(
        &self,
        through: Tag<'_, D::Ids>,
    ) -> Result<DurabilityRequest<D::Ids>, SealError<D::Ids>> {
        self.with(|ring| ring.make_durable_through(through))
    }

    /// [`DurableRing::durable_through`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::durable_through`].
    pub fn durable_through(
        &self,
        lineage: Lin<D>,
    ) -> Result<Option<EpochIdOf<D>>, UnknownLineage<D::Ids>> {
        self.with(|ring| ring.durable_through(lineage))
    }

    /// [`DurableRing::sealed_through`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::sealed_through`].
    pub fn sealed_through(
        &self,
        lineage: Lin<D>,
    ) -> Result<Option<EpochIdOf<D>>, UnknownLineage<D::Ids>> {
        self.with(|ring| ring.sealed_through(lineage))
    }

    /// [`DurableRing::epoch_state`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::epoch_state`].
    pub fn epoch_state(
        &self,
        epoch: Epoch<D::Ids>,
    ) -> Result<EpochState<D::Ids>, UnknownLineage<D::Ids>> {
        self.with(|ring| ring.epoch_state(epoch))
    }

    /// [`DurableRing::resolve`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::resolve`].
    pub fn resolve(
        &self,
        items: Vec<(TokenOf<D>, Resolution)>,
    ) -> Result<(), ResolveError<D::Ids>> {
        self.with(|ring| ring.resolve(items))
    }

    /// [`DurableRing::import_failure`].
    ///
    /// # Errors
    ///
    /// As [`DurableRing::import_failure`].
    pub fn import_failure(
        &self,
        scope: ImportScope<D::Ids>,
    ) -> Result<Imported<D::Ids>, UnknownLineage<D::Ids>> {
        self.with(|ring| ring.import_failure(scope))
    }

    /// [`DurableRing::failures`].
    pub fn failures(&self) -> Vec<FailureInfo<D::Ids>> {
        self.with(|ring| ring.failures())
    }

    /// [`DurableRing::take_token`].
    pub fn take_token(&self, failure: FailureIdOf<D>) -> Option<TokenOf<D>> {
        self.with(|ring| ring.take_token(failure))
    }
}

impl<D: RegisteredBufferRing> DeliveryHandle<D> {
    /// Run `f` on the bytes of registered buffer `i`, under the front end's lock.
    ///
    /// # Errors
    ///
    /// As [`RegisteredBufferRing::with_registered_buffer`].
    pub fn with_registered_buffer<R>(&self, i: u32, f: impl FnOnce(&[u8]) -> R) -> io::Result<R> {
        self.with(|ring| ring.with_registered_buffer(i, f))
    }

    /// Run `f` on the bytes of registered buffer `i`, mutably, under the front end's lock.
    ///
    /// # Errors
    ///
    /// As [`RegisteredBufferRing::with_registered_buffer_mut`].
    pub fn with_registered_buffer_mut<R>(
        &self,
        i: u32,
        f: impl FnOnce(&mut [u8]) -> R,
    ) -> io::Result<R> {
        self.with(|ring| ring.with_registered_buffer_mut(i, f))
    }

    /// [`RegisteredBufferRing::write_registered`].
    ///
    /// # Errors
    ///
    /// As [`RegisteredBufferRing::write_registered`].
    pub fn write_registered<K: WriteKey>(
        &self,
        file: K,
        offset: u64,
        span: RegisteredSpan,
        tag: Tag<'_, D::Ids>,
        context: D::Context,
    ) -> PushResult<D> {
        self.with(|ring| ring.write_registered(file, offset, span, tag, context))
    }

    /// [`RegisteredBufferRing::write_registered_with`].
    ///
    /// # Errors
    ///
    /// As [`RegisteredBufferRing::write_registered_with`].
    pub fn write_registered_with<K: WriteKey>(
        &self,
        file: K,
        offset: u64,
        span: RegisteredSpan,
        tag: Tag<'_, D::Ids>,
        context: D::Context,
        options: WriteOptions<D::Ids>,
    ) -> PushResult<D> {
        self.with(|ring| ring.write_registered_with(file, offset, span, tag, context, options))
    }

    /// [`RegisteredBufferRing::read_registered`].
    ///
    /// # Errors
    ///
    /// As [`RegisteredBufferRing::read_registered`].
    pub fn read_registered<K: ReadKey>(
        &self,
        file: K,
        offset: u64,
        span: RegisteredSpan,
        context: D::Context,
    ) -> PushResult<D> {
        self.with(|ring| ring.read_registered(file, offset, span, context))
    }

    /// [`RegisteredBufferRing::read_registered_with`].
    ///
    /// # Errors
    ///
    /// As [`RegisteredBufferRing::read_registered_with`].
    pub fn read_registered_with<K: ReadKey>(
        &self,
        file: K,
        offset: u64,
        span: RegisteredSpan,
        context: D::Context,
        options: ReadOptions<D::Ids>,
    ) -> PushResult<D> {
        self.with(|ring| ring.read_registered_with(file, offset, span, context, options))
    }
}

/// A failed [`EntryDelivery::new`], handing back what it took.
pub struct DeliverySetupError<D, F> {
    /// Why it failed.
    pub error: io::Error,
    /// The instance.
    pub ring: D,
    /// The handler.
    pub on_entry: F,
}

impl<D, F> fmt::Debug for DeliverySetupError<D, F> {
    // By hand, so neither the instance nor the handler need be `Debug`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeliverySetupError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// The Model A front end: owns an instance and hands its entries, one at a time and in queue
/// order, to a handler on thread-pool threads. Derefs to the [`DeliveryHandle`] through which the
/// instance's operations are reached. For an instance whose readiness signal is an [`Event`],
/// which it waits on through the thread pool.
///
/// # The handler
///
/// Called with a [`DeliveryEvent`]: each entry, in queue order, and a failure of the instance's
/// `pop` when it has one to report, which a handler that ignores is a delivery that stalls
/// without saying so. Called on a pool thread, never twice at once, with the instance's lock released. It must not
/// panic -- a panic unwinds into the thread pool's callback and aborts the process -- and while it
/// runs no further entry is delivered, so a handler that blocks holds up delivery, and holds up
/// [`into_inner`](Self::into_inner) and drop, which wait for it.
///
/// # Ending
///
/// Drop, [`close`](Self::close) and [`into_inner`](Self::into_inner) all quiesce the delivery
/// first: no handler call is running or will start (DI-2.7 point 5), so none runs once they have
/// returned. Drop then drops the instance, and `close` closes it, each discarding any entry not yet
/// delivered; `into_inner` hands the instance back with those entries still queued, so it can be
/// popped directly or closed.
pub struct EntryDelivery<D: DurableRing<Readiness = Event> + Send + 'static> {
    /// `None` once the delivery has been quiesced.
    wait: Option<ThreadpoolWait>,
    inner: Arc<Inner<D, Handler<D>>>,
}

impl<D: DurableRing<Readiness = Event> + Send + 'static> EntryDelivery<D> {
    /// Take `ring` and deliver its entries to `on_entry`, on the default thread pool, or on the
    /// pool and priority `env` selects.
    ///
    /// Entries already queued when the ring is handed over are delivered: the readiness signal may
    /// already have been consumed, so this arms the wait and then sets the signal itself.
    ///
    /// # Errors
    ///
    /// A [`DeliverySetupError`] handing back the instance and the handler, when duplicating the
    /// readiness signal, creating the thread-pool wait, or setting the signal fails. Only the first
    /// is reached by a test: the others fail when the process cannot get another handle or the
    /// thread pool cannot create a wait.
    pub fn new<F>(
        mut ring: D,
        on_entry: F,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> Result<Self, DeliverySetupError<D, F>>
    where
        F: FnMut(DeliveryEvent<EntryOf<D>>, &DeliveryHandle<D>) + Send + 'static,
    {
        let duplicates = ring
            .readiness()
            .and_then(|signal| Ok((signal.try_clone()?, signal)));
        let (signal, kick) = match duplicates {
            Ok(duplicates) => duplicates,
            Err(error) => {
                return Err(DeliverySetupError {
                    error,
                    ring,
                    on_entry,
                });
            }
        };
        let inner: Arc<Inner<D, F>> = Arc::new(Inner {
            handle: DeliveryHandle {
                ring: Mutex::new(Some(ring)),
            },
            handler: Mutex::new(on_entry),
        });

        let deliverer: Arc<Inner<D, Handler<D>>> = inner.clone();
        let wait = match ThreadpoolWait::new(
            signal.into(),
            move |activation| deliver(&deliverer, activation),
            env,
        ) {
            Ok(wait) => wait,
            // The refused wait dropped its callback, and the deliverer's reference with it.
            Err(error) => return Err(recover(inner, error)),
        };
        wait.arm(None);
        if let Err(error) = kick.set() {
            wait.stop_and_drain();
            drop(wait);
            return Err(recover(inner, error));
        }
        Ok(Self {
            wait: Some(wait),
            inner,
        })
    }

    /// Quiesce the delivery and hand back the instance, with any entry not yet delivered still in
    /// its queue.
    #[must_use]
    pub fn into_inner(mut self) -> D {
        self.quiesce();
        self.inner.handle.take()
    }

    /// Quiesce the delivery and end the instance with [`DurableRing::close`], handing back what is
    /// left. Entries not yet delivered are discarded with it.
    pub fn close(self) -> Leftovers<D::Ids, D::Buffer, D::Context> {
        self.into_inner().close()
    }

    /// Disarm the wait, wait for a delivery in progress to finish, and release the wait and the
    /// deliverer's reference to the instance.
    fn quiesce(&mut self) {
        if let Some(wait) = self.wait.take() {
            wait.stop_and_drain();
        }
    }
}

impl<D: DurableRing<Readiness = Event> + Send + 'static> Deref for EntryDelivery<D> {
    type Target = DeliveryHandle<D>;

    fn deref(&self) -> &DeliveryHandle<D> {
        &self.inner.handle
    }
}

impl<D: DurableRing<Readiness = Event> + Send + 'static> Drop for EntryDelivery<D> {
    fn drop(&mut self) {
        // Before the instance is released, which the field drop that follows does (DI-2.7 point
        // 5): no handler call can still be reaching it.
        self.quiesce();
    }
}

/// One wake: pop to `None`, handing each entry to the handler with the instance's lock released,
/// then re-arm (DI-D-28). Re-arming last is what serialises delivery: a wake that arrives while
/// this one drains finds the signal set once it is re-armed, and starts only after this one has
/// made its last handler call.
///
/// A failed `pop` is the handler's to be told, once, and ends the wake: dioring's is a submission
/// its doorbell could not issue, or a ring it could not read, with nothing to pop, and the readiness
/// edge this wake took is the only thing that told the consumer (DI-D-58). Looping again would
/// repeat a cause that stays, with nothing to stop it.
fn deliver<D: DurableRing>(inner: &Inner<D, Handler<D>>, activation: &WaitActivation<'_>) {
    loop {
        let event = match inner.handle.with(|ring| ring.pop()) {
            Ok(Some(entry)) => DeliveryEvent::Entry(entry),
            Ok(None) => break,
            Err(error) => {
                let mut handler = lock(&inner.handler);
                (*handler)(DeliveryEvent::Failure(error), &inner.handle);
                break;
            }
        };
        let mut handler = lock(&inner.handler);
        (*handler)(event, &inner.handle);
    }
    activation.rearm(None);
}

fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The handler aborts the process rather than unwinding, so a poisoned handler lock means a
    // panic elsewhere, which leaves the handler as it was.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Take back the instance and the handler from a front end that never started delivering.
fn recover<D, F>(inner: Arc<Inner<D, F>>, error: io::Error) -> DeliverySetupError<D, F> {
    let Inner { handle, handler } = Arc::into_inner(inner)
        .expect("no wait holds the deliverer's reference once construction has failed");
    DeliverySetupError {
        error,
        ring: handle
            .ring
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .expect("the instance is still in place"),
        on_entry: handler.into_inner().unwrap_or_else(PoisonError::into_inner),
    }
}
