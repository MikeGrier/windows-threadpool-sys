// Copyright (c) 2026 Mike Grier
//! dioring: the contract's implementation over `windows-ioring-sys` (DI-D-49, DI-D-53).
//!
//! An instance is a [`DurableIoCore`] over a [`WindowsBackend`], joined by an [`Engine`] that its
//! delivery drains whenever the ring's completion event says to: the core decides, the backend runs
//! the I/O, and the driver here joins them to the consumer's calls. **Delivery is chosen at
//! construction** ([`DeliveryMode`], DI-D-55): drained on the thread pool, or by the consumer's own
//! thread in [`Dioring::pump`]. Every path -- a consumer's call, a drain, a provider's answer, a
//! lineage handle's release -- takes the engine's one lock, so the queue's order is the order
//! completions were recorded in, whichever thread recorded them.
//!
//! **The readiness event** is set whenever a path made entries poppable where there were none, after
//! it releases the lock, so they are poppable before it is set (DI-D-28).
//!
//! **The consumer's provider** is called at the end of the consumer's own calls, with the lock
//! released, so it may answer inside its call (DI-D-44). A request a pool drain, an answer or a
//! release makes waits in the engine for the consumer's next call, normally the `pop` that follows.
//!
//! **Ending** -- `close()` or drop -- stops delivery and takes the engine out of its slot, so an
//! answer or a release that comes later finds nothing; drops the provider, so a completion it keeps
//! is answered as abandoned; waits until every completion made has been answered or dropped
//! (DI-D-25); then closes the core and drops the backend, whose ring runs down every operation still
//! in flight. An internal inconsistency is not looked for here: it is panicked on where it is found
//! (DI-D-46).

use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::{Arc, Weak};
use std::time::Duration;

use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::{Event, ResetMode};
use win_time_sys::{Clock, InterruptClock, InterruptTime, Steady, TimePoint};
use windows_ioring_sys::{
    Batch, Completion, IoBuf, IoBufMut, IoRing, PushRefused, RegisteredBuffers, RegisteredFiles,
    RegisteredSpan,
};
use windows_threadpool_sys::callback_env::CallbackEnviron;

use crate::error_code::windows_fault;
use crate::windows_backend::pool::{PoolDelivery, PoolEngine};
use crate::windows_backend::signalled::SignalledDelivery;
use crate::windows_backend::{
    Construction, Win32FileTarget, WindowsBackend, WindowsBackendError, WindowsBackendRefusal,
    WindowsBackendSetup,
};
use durable_ioring::DiagnosticSink;
use durable_ioring::DurableIoCore;
use durable_ioring::IoTarget;
use durable_ioring::StampClock;
use durable_ioring::backend::{IoBackend, RegisteredRegion, SubmissionData, refusal_fault};
use durable_ioring::driver::Home;
use durable_ioring::driver::LockWatch;
use durable_ioring::driver::PumpedDelivery;
use durable_ioring::driver::first_repeat;
use durable_ioring::driver::{Answer, DomainCall};
use durable_ioring::driver::{Engine, EngineGuard};
use durable_ioring::{
    Access, AddTargetCause, AddTargetError, DurabilityRequest, EndLineageError, EndLineageRefusal,
    Epoch, EpochState, FailureInfo, FlushDomain, ImportScope, Imported, IoFault, IoTargetKey,
    Leftovers, LineageInfo, ReadOptions, RemoveTargetError, Resolution, ResolveError,
    RetireLineageError, RetireLineageRefusal, SealError, Tag, UnknownLineage, WriteOptions,
};
use durable_ioring::{DioringIds, FailureId, FailureToken, Lineage, LineageHandle};
use durable_ioring::{
    DomainCompletion, DomainRequest, DurabilityProvider, FlushRequest, ProviderPart,
    ProviderRequest,
};
use durable_ioring::{DurableRing, EntryOf, EpochId, PushResult, RegisteredBufferRing};
use durable_ioring::{ReadKey, TargetKey, WriteKey};
use durable_ioring_events::EtwDiagnostics;

mod delivery;
mod hub;

use delivery::Delivery;
use hub::Hub;

// `pub(crate)` so other modules' tests can build an instance with the helpers here.
#[cfg(test)]
pub(crate) mod tests;

/// A clock dioring can stamp with: a [`StampClock`] whose readings are interrupt time, its one time
/// base (DI-D-37), and never decrease, and that can be held under the lock pool threads take
/// (DI-D-38). [`InterruptStamps`] is the default; a test may give an instance its own mock, which
/// takes on `StampClock`'s promise by implementing it, and a `win-time-sys` clock on interrupt
/// time is given as [`FromClock`]. Implemented for every type that meets those bounds.
pub trait TimeBase: StampClock<Instant = WinInstant> {}

impl<K: StampClock<Instant = WinInstant>> TimeBase for K {}

/// The time type the Windows driver's stamps are read in: interrupt time, its one time base
/// (DI-D-37).
pub type WinInstant = TimePoint<InterruptTime>;

/// dioring's identities over the Windows driver's stamps (DI-D-50): what a consumer names its
/// [`Dioring`]'s identities by.
pub type WindowsIds<E> = DioringIds<E, WinInstant>;

/// Interrupt time, read through `win-time-sys`' `InterruptClock`: the default clock a [`Dioring`]
/// stamps with (DI-D-37). Its readings never decrease, which is `StampClock`'s promise.
#[derive(Clone, Copy, Debug, Default)]
pub struct InterruptStamps;

impl StampClock for InterruptStamps {
    type Instant = WinInstant;

    fn now(&self) -> WinInstant {
        Clock::now(&InterruptClock)
    }
}

/// A `win-time-sys` clock on interrupt time, given to dioring as the clock it stamps with: the
/// clock must keep [`Steady`]'s promise that its readings never decrease, which it states by
/// implementing it. A `win-time-sys` clock cannot be a [`StampClock`] itself, because the trait is
/// another crate's.
#[derive(Clone, Copy, Debug, Default)]
pub struct FromClock<K>(pub K);

impl<K: Clock<Timeline = InterruptTime> + Steady + Send + 'static> StampClock for FromClock<K> {
    type Instant = WinInstant;

    fn now(&self) -> WinInstant {
        Clock::now(&self.0)
    }
}

/// How long construction waits for each registration's completion. A registration is a
/// bookkeeping operation the kernel completes at once; the bound exists so a ring that never
/// answers fails construction instead of hanging it.
pub(crate) const REGISTRATION_BOUND: Duration = Duration::from_secs(30);

/// What a dioring instance is built with. Both registrations are the ring's only ones (one of
/// each per ring), so they are made here or never.
pub struct Setup<'env, 'pool, E: EpochId + 'static, R> {
    /// The ring's submission queue size, as `windows-ioring-sys` takes it.
    pub submission_queue_size: u32,
    /// The ring's completion queue size, as `windows-ioring-sys` takes it.
    pub completion_queue_size: u32,
    /// Registered with the ring, which holds their files for its life. A target added later is not
    /// registered.
    pub files: Vec<TargetSetup>,
    /// Already allocated and placed by the consumer; dioring neither allocates nor places them.
    /// Empty registers nothing.
    pub buffers: Vec<R>,
    /// The consumer's durability provider, if any. `None` leaves every domain to the built-in
    /// default.
    pub provider: Option<Box<dyn DurabilityProvider<WindowsIds<E>>>>,
    /// Where the instance's diagnostics go (DI-D-51). `None` is the driver's default, [`EtwDiagnostics`]: the instance's begin and
    /// end events go to the ETW provider `durable-ioring-events`, for a session that is listening, and cost a test of whether one
    /// is when none is. Any sink can be given in its place, [`NoDiagnostics`](durable_ioring::NoDiagnostics) among them.
    /// Building an instance with the default registers the provider for the process, if no instance has.
    pub diagnostics: Option<Arc<dyn DiagnosticSink>>,
    /// How the instance's completions are drained (DI-D-52, DI-D-55).
    pub delivery: DeliveryMode<'env, 'pool>,
}

/// How an instance's completions are drained: by the thread pool, or by the consumer's own thread,
/// which the pool may tell. Chosen once, as the instance is built, and not changed after.
pub enum DeliveryMode<'env, 'pool> {
    /// Drained on a pool thread whenever the ring's completion event says to, and the consumer
    /// told through [`DurableRing::readiness`]. Holds the thread pool, and the priority within it,
    /// that delivery runs on -- a NUMA node's own pool, for one; `None` is the process's default
    /// pool. Read once, as the instance is built.
    Pool(Option<&'env mut CallbackEnviron<'pool>>),
    /// Drained by the consumer's own thread, which calls [`Dioring::pump`]: no thread of the
    /// instance's own, and none of the pool's. Nothing is drained until it does. Its pump waits on
    /// the ring's own completion event, a handle to which [`Dioring::completion_event`] offers
    /// the consumer, to wait on beside its own handles before it pumps.
    Pumped,
    /// Drained by the consumer's own thread, as [`Pumped`](Self::Pumped), which the thread pool
    /// tells: it sets an event of the instance's own, [`Dioring::completion_signal`], whenever
    /// completions may be waiting, and the consumer waits on that beside its own handles and then
    /// calls [`Dioring::pump`]. A thread-pool wait holds no thread while it waits; its callback only
    /// sets the event. Holds the thread pool, and the priority within it, that callback runs on,
    /// as [`Pool`](Self::Pool)'s does.
    Signalled(Option<&'env mut CallbackEnviron<'pool>>),
}

/// One target given at construction.
#[derive(Debug)]
pub struct TargetSetup {
    /// The consumer's key for it, unique within the instance.
    pub key: IoTargetKey,
    /// The target: its file, and what it states, flush domains among it.
    pub target: Win32FileTarget,
    /// The access the consumer's key declares for it, if the key was a typed one (DI-13.10).
    /// Construction is refused with [`SetupRefusal::AccessNotCovered`] when the target states
    /// less.
    pub declared: Option<Access>,
}

impl TargetSetup {
    /// `target` under `key`, which may be a [`Key<A>`](durable_ioring::Key) declaring what the
    /// target can do, or an untyped [`IoTargetKey`], which declares nothing.
    #[must_use]
    pub fn new<K: TargetKey>(key: K, target: Win32FileTarget) -> Self {
        Self {
            key: key.untyped(),
            declared: key.declared(),
            target,
        }
    }

    /// What construction refuses this target for, if it declared more than it states.
    fn refusal(&self) -> Option<SetupRefusal> {
        let declared = self.declared?;
        let actual = self.target.access();
        (!actual.covers(declared)).then_some(SetupRefusal::AccessNotCovered {
            key: self.key,
            declared,
            actual,
        })
    }
}

/// Why construction failed.
#[derive(Debug)]
pub enum SetupRefusal {
    /// Two targets share a key.
    DuplicateKey(IoTargetKey),
    /// A target's key declared more access than the target states (DI-13.10).
    AccessNotCovered {
        /// The key.
        key: IoTargetKey,
        /// What the key declared.
        declared: Access,
        /// What the target states.
        actual: Access,
    },
    /// The provider named a domain twice.
    DuplicateProviderDomain(FlushDomain),
    /// Creating the ring, making a registration, or wiring the ring's delivery failed.
    Ring(io::Error),
}

/// A failed construction hands back what it was given. `buffers` is `None` only once the buffers
/// have been handed to the kernel and could not be shown to have come back: when the kernel's
/// registration completion itself failed (`windows-ioring-sys` drops them then), when no
/// completion arrived (the ring crate leaks them rather than free memory the kernel may hold), or
/// when the registration succeeded and wiring the ring's delivery then failed (the registration
/// holds them, and has no way to give them back).
///
/// That last failure is not reached by this crate's tests: wiring delivery fails where the system
/// does not support a ring completion event, or the thread pool cannot create a wait.
pub struct SetupError<E: EpochId + 'static, R> {
    /// Why construction failed.
    pub reason: SetupRefusal,
    /// The targets, in the order given.
    pub files: Vec<TargetSetup>,
    /// The buffers, unless the kernel was given them; see above.
    pub buffers: Option<Vec<R>>,
    /// The provider, if one was given.
    pub provider: Option<Box<dyn DurabilityProvider<WindowsIds<E>>>>,
}

impl<E: EpochId + 'static, R> std::fmt::Debug for SetupError<E, R> {
    // By hand, so no bound is placed on `R`: a buffer type need not be `Debug` for `.expect(..)`
    // to compile.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupError")
            .field("reason", &self.reason)
            .field("files", &self.files)
            .field("buffers", &self.buffers.as_ref().map(Vec::len))
            .field("provider", &self.provider)
            .finish()
    }
}

/// dioring: `B` is the owned buffer type, `E` the epoch-id type, `C` the consumer's
/// per-operation context, `R` the registered-buffer type, and `K` the clock it stamps failures with
/// (a [`TimeBase`]; [`InterruptStamps`] unless built with [`Dioring::with_clock`]).
///
/// The instance's core and its Windows backend are drained on the thread pool (DI-D-53), so the
/// buffer, epoch-id, context and registered-buffer types cross to pool threads and must be `Send`;
/// the epoch-id type must also be `Sync`, because a failure's suspect set is shared. An owned write
/// needs only `B: IoBuf`, so a write-only type serves; an owned read needs `B: IoBufMut`
/// (DI-D-17).
pub struct Dioring<
    B: IoBuf,
    E: EpochId + 'static = u64,
    C = (),
    R: IoBufMut = Vec<u8>,
    K: StampClock = InterruptStamps,
> {
    /// The core and its backend, and how they are drained; `None` once the instance has ended.
    pub(crate) delivery: Option<Delivery<E, B, C, K, R>>,
    pub(crate) hub: Arc<Hub<E, B, C, K, R>>,
    pub(crate) provider: Option<Box<dyn DurabilityProvider<WindowsIds<E>>>>,
    /// A copy of the default lineage's handle, so the default lineage lives as long as the
    /// instance does (DI-D-40).
    pub(crate) default: LineageHandle,
}

impl<B, E, C, R> Dioring<B, E, C, R>
where
    B: IoBuf + Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut + Send + 'static,
{
    /// Build an instance, make the ring's registrations, and wire its delivery. Blocks until both
    /// registrations have completed. Returns the instance with a handle to its default lineage
    /// (DI-D-40). Failures are stamped with [`InterruptStamps`]; see [`Dioring::with_clock`] for
    /// another clock.
    ///
    /// # Errors
    ///
    /// A [`SetupError`] handing back what it was given, for a duplicate target key, a provider
    /// domain named twice, or a ring that could not be created, make a registration, or be wired
    /// to deliver its completions. The checks that need no ring run first, so a refusal for
    /// either of the first two creates nothing.
    pub fn new(setup: Setup<'_, '_, E, R>) -> Result<(Self, LineageHandle), SetupError<E, R>> {
        Self::with_clock(setup, InterruptStamps)
    }
}

impl<B, E, C, R, K> Dioring<B, E, C, R, K>
where
    B: IoBuf + Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut + Send + 'static,
    K: TimeBase,
{
    /// [`Dioring::new`], stamping failures with `clock` (DI-D-38): a test's mock, for one, which
    /// must keep [`StampClock`]'s promise that its readings never decrease.
    ///
    /// # Errors
    ///
    /// As [`Dioring::new`].
    pub fn with_clock(
        setup: Setup<'_, '_, E, R>,
        clock: K,
    ) -> Result<(Self, LineageHandle), SetupError<E, R>> {
        Self::with_construction(setup, clock, Construction::default())
    }

    /// [`with_clock`](Self::with_clock), as `construction` says (DI-7.2).
    ///
    /// # Errors
    ///
    /// As [`Dioring::new`].
    pub(crate) fn with_construction(
        setup: Setup<'_, '_, E, R>,
        clock: K,
        construction: Construction<B>,
    ) -> Result<(Self, LineageHandle), SetupError<E, R>> {
        let Setup {
            submission_queue_size,
            completion_queue_size,
            files,
            buffers,
            provider,
            diagnostics,
            delivery,
        } = setup;
        let diagnostics = diagnostics.unwrap_or_else(|| Arc::new(EtwDiagnostics::new()));

        if let Some(key) = first_repeat(files.iter().map(|f| f.key)) {
            return Err(SetupError {
                reason: SetupRefusal::DuplicateKey(key),
                files,
                buffers: Some(buffers),
                provider,
            });
        }
        if let Some(reason) = files.iter().find_map(TargetSetup::refusal) {
            return Err(SetupError {
                reason,
                files,
                buffers: Some(buffers),
                provider,
            });
        }
        let provider_domains = provider.as_ref().map(|p| p.domains()).unwrap_or_default();
        if let Some(domain) = first_repeat(provider_domains.iter().cloned()) {
            return Err(SetupError {
                reason: SetupRefusal::DuplicateProviderDomain(domain),
                files,
                buffers: Some(buffers),
                provider,
            });
        }

        let signals =
            Event::new(ResetMode::Auto, false).map(|readiness| (readiness.clone(), readiness));
        let (ready, readiness) = match signals {
            Ok(signals) => signals,
            Err(error) => {
                return Err(SetupError {
                    reason: SetupRefusal::Ring(error),
                    files,
                    buffers: Some(buffers),
                    provider,
                });
            }
        };
        let mut core = if provider.is_some() {
            DurableIoCore::with_provider(clock, Arc::clone(&diagnostics), provider_domains)
                .expect("a domain named twice was refused above")
        } else {
            DurableIoCore::new(clock, Arc::clone(&diagnostics))
        };
        let keys: Vec<IoTargetKey> = files.iter().map(|f| f.key).collect();
        // Kept so that a failed construction hands the files back as they were given.
        let declarations: Vec<(IoTargetKey, Access)> = files
            .iter()
            .filter_map(|f| f.declared.map(|declared| (f.key, declared)))
            .collect();
        for file in &files {
            core.add_target(file.key, file.target.describe())
                .expect("the keys were checked unique above");
        }
        let targets = files
            .into_iter()
            .map(|TargetSetup { key, target, .. }| (key, target))
            .collect();
        let backend = match WindowsBackend::construct(
            WindowsBackendSetup {
                submission_queue_size,
                completion_queue_size,
                targets,
                buffers,
            },
            construction,
        ) {
            Ok(backend) => backend,
            Err(WindowsBackendError {
                reason,
                targets,
                buffers,
            }) => {
                let reason = match reason {
                    WindowsBackendRefusal::DuplicateKey(key) => SetupRefusal::DuplicateKey(key),
                    WindowsBackendRefusal::Ring(error) => SetupRefusal::Ring(error),
                };
                return Err(SetupError {
                    reason,
                    files: setups(targets, &declarations),
                    buffers,
                    provider,
                });
            }
        };

        let default_lineage = core.default_lineage();
        let watch = LockWatch::new(default_lineage.instance.number(), Arc::clone(&diagnostics));
        let on_ready = move || {
            // Setting an event this instance created, with full access, does not fail.
            let set = ready.set();
            debug_assert!(set.is_ok(), "setting the readiness event failed: {set:?}");
        };
        let engine = Engine::new(core, backend);
        let delivered = match delivery {
            DeliveryMode::Pool(environment) => {
                PoolDelivery::new(engine, on_ready, environment, watch.clone()).map(Delivery::Pool)
            }
            // Nothing sets the signal the pool's wake would: entries are made poppable by the
            // pump, which sets the readiness event itself, as every other path does.
            DeliveryMode::Pumped => PumpedDelivery::new(engine)
                .map_err(|(fault, engine)| (io::Error::from(fault), engine))
                .map(|pumped| pumped.with_lock_watch(watch.clone()))
                .map(Delivery::Pumped),
            DeliveryMode::Signalled(environment) => {
                SignalledDelivery::new(engine, environment, watch.clone()).map(Delivery::Signalled)
            }
        };
        let delivery = match delivered {
            Ok(delivery) => delivery,
            Err((error, engine)) => {
                let mut backend = engine.backend;
                let files = keys
                    .into_iter()
                    .filter_map(|key| {
                        backend.detach(key).map(|target| TargetSetup {
                            key,
                            target,
                            declared: declared_for(&declarations, key),
                        })
                    })
                    .collect();
                return Err(SetupError {
                    reason: SetupRefusal::Ring(error),
                    files,
                    buffers: None,
                    provider,
                });
            }
        };
        let hub = Arc::new(Hub::new(
            delivery.shared(),
            readiness,
            default_lineage.instance,
            diagnostics,
            watch,
        ));
        let default = LineageHandle::mint(default_lineage, home(&hub));
        let handle = default.clone();
        Ok((
            Self {
                delivery: Some(delivery),
                hub,
                provider,
                default,
            },
            handle,
        ))
    }
}

/// Targets handed back as they were given.
fn setups(
    targets: Vec<(IoTargetKey, Win32FileTarget)>,
    declarations: &[(IoTargetKey, Access)],
) -> Vec<TargetSetup> {
    targets
        .into_iter()
        .map(|(key, target)| TargetSetup {
            key,
            target,
            declared: declared_for(declarations, key),
        })
        .collect()
}

/// What the key declared at construction, if it was a typed key.
fn declared_for(declarations: &[(IoTargetKey, Access)], key: IoTargetKey) -> Option<Access> {
    declarations
        .iter()
        .find_map(|&(given, access)| (given == key).then_some(access))
}

/// Where an instance's lineage handles report their release.
fn home<T: Home + 'static>(hub: &Arc<T>) -> Weak<dyn Home> {
    let home: Weak<T> = Arc::downgrade(hub);
    home
}

/// Where an instance's provider completions answer.
fn answers<T: Answer + 'static>(hub: &Arc<T>) -> Weak<dyn Answer> {
    let answers: Weak<T> = Arc::downgrade(hub);
    answers
}

impl<B, E, C, R, K> DurableRing for Dioring<B, E, C, R, K>
where
    B: IoBuf + Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut + Send + 'static,
    K: TimeBase,
{
    type Ids = WindowsIds<E>;
    type Buffer = B;
    type Context = C;
    type Readiness = Event;
    type Target = Win32FileTarget;

    fn readiness(&mut self) -> Result<Event, IoFault> {
        self.hub.readiness().map_err(windows_fault)
    }

    fn add_target<T: TargetKey>(
        &mut self,
        declared: T,
        target: Win32FileTarget,
    ) -> Result<(), AddTargetError<Win32FileTarget>> {
        let key = declared.untyped();
        let mut engine = self.engine();
        let cause = if engine.core.target(key).is_some() || engine.backend.target(key).is_some() {
            Some(AddTargetCause::KeyInUse)
        } else {
            declared.admitted_by(target.access()).err()
        };
        if let Some(cause) = cause {
            return Err(AddTargetError { key, target, cause });
        }
        engine
            .core
            .add_target(key, target.describe())
            .expect("the key is free, checked above");
        engine
            .backend
            .attach(key, target)
            .expect("the key is free, checked above");
        Ok(())
    }

    fn remove_target(
        &mut self,
        key: IoTargetKey,
    ) -> Result<Win32FileTarget, RemoveTargetError<Self::Ids>> {
        let mut engine = self.engine();
        engine.core.remove_target(key)?;
        Ok(engine
            .backend
            .detach(key)
            .expect("a target the core decided for is attached"))
    }

    fn write_with<T: WriteKey>(
        &mut self,
        file: T,
        offset: u64,
        buffer: B,
        tag: Tag<'_, Self::Ids>,
        context: C,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        B: IoBuf,
    {
        let (pushed, attention) = {
            let mut engine = self.engine();
            let pushed = engine.write(
                file.untyped(),
                offset,
                SubmissionData::Owned(buffer),
                tag.epoch(),
                context,
                options,
            );
            (pushed, engine.take_attention())
        };
        self.attend(attention);
        pushed
    }

    fn read_with<T: ReadKey>(
        &mut self,
        file: T,
        offset: u64,
        buffer: B,
        context: C,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        B: IoBufMut,
    {
        let (pushed, attention) = {
            let mut engine = self.engine();
            // Here, where `B: IoBufMut` holds, the backend is given the push an owned read needs
            // (DI-D-17).
            engine.backend.enable_owned_reads();
            let pushed = engine.read(
                file.untyped(),
                offset,
                SubmissionData::Owned(buffer),
                context,
                options,
            );
            (pushed, engine.take_attention())
        };
        self.attend(attention);
        pushed
    }

    fn pop(&mut self) -> Result<Option<EntryOf<Self>>, IoFault> {
        // A push is submitted as it is made, by the engine's doorbell, so what is left to submit
        // here is what a failed one left queued, and the driver told the consumer so (DI-7+.2). It
        // is reported only when there is no entry to return instead: an entry is progress, and
        // the submission is tried again at the next pop. A fault the backend gave when its
        // completions were read is reported the same way, once: the consumer's remedy for either
        // that stays is to drop the instance.
        //
        // A submission of the core's own the backend refused needs nothing here: its cause is
        // cleared by a submit or a drain, and either way completions are still to come, whose
        // actions the pool drain re-emits it with.
        //
        // The failed-submit report is reached by `dioring/tests/submits.rs`, through a responder
        // registered for this ring, which fails the submits pool threads make as well (DI-7.1).
        let popped = {
            let mut engine = self.engine();
            let submitted = engine.backend.submit();
            match engine.core.pop() {
                Some(entry) => Ok(Some(entry)),
                None => match (engine.take_fault(), submitted) {
                    (Some(fault), _) => Err(fault),
                    (None, Err(cause)) => Err(refusal_fault(cause)),
                    (None, Ok(())) => Ok(None),
                },
            }
        };
        // What a drain asked of the provider waits for a consumer call, and a pop is one (DI-D-44).
        self.call_provider();
        popped
    }

    fn make_durable_through(
        &mut self,
        through: Tag<'_, Self::Ids>,
    ) -> Result<DurabilityRequest<Self::Ids>, SealError<Self::Ids>> {
        let sealed = self.engine().make_durable_through(through.epoch());
        let (answer, deliver) = sealed?;
        self.settled(deliver);
        Ok(answer)
    }

    fn durable_through(&self, lineage: Lineage) -> Result<Option<E>, UnknownLineage<Self::Ids>> {
        self.engine().core.durable_through(lineage)
    }

    fn sealed_through(&self, lineage: Lineage) -> Result<Option<E>, UnknownLineage<Self::Ids>> {
        self.engine().core.sealed_through(lineage)
    }

    fn epoch_state(
        &self,
        epoch: Epoch<Self::Ids>,
    ) -> Result<EpochState<Self::Ids>, UnknownLineage<Self::Ids>> {
        self.engine().core.epoch_state(epoch)
    }

    fn default_lineage(&self) -> LineageHandle {
        self.default.clone()
    }

    fn mint_lineage(&mut self, description: Option<String>) -> LineageHandle {
        let lineage = self.engine().core.mint_lineage(description);
        LineageHandle::mint(lineage, home(&self.hub))
    }

    fn lineages(&self) -> Vec<LineageInfo<Self::Ids>> {
        self.engine().core.lineages()
    }

    fn end_lineage(&mut self, handle: LineageHandle) -> Result<(), EndLineageError<Self::Ids>> {
        let lineage = handle.lineage();
        let reason = if !self.owns(&handle) {
            EndLineageRefusal::Foreign(lineage)
        } else if lineage == self.default.lineage() {
            EndLineageRefusal::Default
        } else {
            match handle.take_last() {
                Ok(lineage) => {
                    let ended = {
                        let mut engine = self.engine();
                        let ended = engine.core.end_lineage(lineage);
                        ended.map(|actions| engine.act(actions))
                    };
                    match ended {
                        Ok(deliver) => self.settled(deliver),
                        // Live, since this was its last handle and taking it disarmed it: a
                        // refusal means a release ended it already.
                        Err(refusal) => self.hub.inconsistent(
                            "the last handle of a lineage found it already ended",
                            format_args!("lineage {lineage:?}, refused as {refusal:?}"),
                        ),
                    }
                    return Ok(());
                }
                Err(handle) => {
                    return Err(EndLineageError {
                        reason: EndLineageRefusal::Shared,
                        handle,
                    });
                }
            }
        };
        Err(EndLineageError { reason, handle })
    }

    fn retire_lineage(
        &mut self,
        handle: LineageHandle,
    ) -> Result<(), RetireLineageError<Self::Ids>> {
        let lineage = handle.lineage();
        let reason = if !self.owns(&handle) {
            RetireLineageRefusal::Foreign(lineage)
        } else if lineage == self.default.lineage() {
            RetireLineageRefusal::Default
        } else if !handle.is_last() {
            RetireLineageRefusal::Shared
        } else {
            // Unlocked before the handle is consumed: a handle's release takes this lock.
            let retired = self.engine().core.retire_lineage(lineage);
            match retired {
                Err(refusal) => refusal,
                Ok(()) => {
                    // The last copy, owned here, so none can be made.
                    let taken = handle.take_last();
                    debug_assert!(taken.is_ok(), "the last handle, owned here");
                    return Ok(());
                }
            }
        };
        Err(RetireLineageError { reason, handle })
    }

    fn resolve(
        &mut self,
        items: Vec<(FailureToken, Resolution)>,
    ) -> Result<(), ResolveError<Self::Ids>> {
        let resolved = {
            let mut engine = self.engine();
            engine
                .core
                .resolve(items)
                .map(|actions| engine.act(actions))
        };
        let deliver = resolved?;
        self.settled(deliver);
        Ok(())
    }

    fn import_failure(
        &mut self,
        scope: ImportScope<Self::Ids>,
    ) -> Result<Imported<Self::Ids>, UnknownLineage<Self::Ids>> {
        let imported = {
            let mut engine = self.engine();
            engine
                .core
                .import_failure(scope)
                .map(|(imported, actions)| (imported, engine.act(actions)))
        };
        let (imported, deliver) = imported?;
        self.settled(deliver);
        Ok(imported)
    }

    fn failures(&self) -> Vec<FailureInfo<Self::Ids>> {
        self.engine().core.failures()
    }

    fn take_token(&mut self, failure: FailureId) -> Option<FailureToken> {
        self.engine().core.take_token(failure)
    }

    fn close(mut self) -> Leftovers<Self::Ids, B, C> {
        // `close` consumes the instance, so nothing has ended it before.
        self.end().unwrap_or_else(|| Leftovers {
            held: Vec::new(),
            failures: Vec::new(),
            lineages: Vec::new(),
        })
    }
}

impl<B, E, C, R, K> Dioring<B, E, C, R, K>
where
    B: IoBuf + Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut + Send + 'static,
    K: TimeBase,
{
    /// Drain what the ring has completed, on this thread (DI-D-55): drain it to empty and, if that
    /// delivered entries, return; otherwise wait up to `timeout` -- `None` is no limit, and
    /// `Duration::ZERO` is a pump that does not wait -- and drain again. Then make the provider's
    /// requests, and set the readiness event if entries became poppable. Whether they did. Pop what
    /// became poppable with [`DurableRing::pop`].
    ///
    /// **Only for an instance drained by its consumer**, built with [`DeliveryMode::Pumped`] or
    /// [`DeliveryMode::Signalled`]: a completion waits in the ring until a pump, so an instance that
    /// is never pumped delivers nothing. A pumped instance waits on the ring's own completion
    /// event, and a signalled one on its [`completion_signal`](Self::completion_signal), which the
    /// consumer may be waiting on too. **It drains before it waits**, so a wait of the consumer's
    /// own that took the signal cannot leave a pump blocked while completions sit in the ring. The
    /// wait is made with the engine's lock released, so another thread's push, a provider's answer or
    /// a lineage handle's release is not held up by it.
    ///
    /// A return does not promise entries: the wait is satisfied by a raising whose completions an
    /// earlier drain took, and by the one signal the ring crate raises as it attaches the event
    /// ([D-20](../../windows-ioring-sys/DESIGN-NOTES.md#d-20)), so a first pump may return at once.
    /// Pump in a loop, and pop to empty each time. With no limit and nothing in flight a pump never
    /// returns.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] for an instance built with [`DeliveryMode::Pool`], which the
    /// pool drains. Otherwise the wait's own error, when the event cannot be waited on; the drain is
    /// made all the same, so what has completed is delivered, and the entries it made poppable are
    /// there to pop.
    pub fn pump(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        let Some(pumped) = self
            .delivery
            .as_mut()
            .expect("an instance is live until it ends")
            .pump(timeout)
        else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the instance was built with pool delivery, which drains it; pumping is for \
                 DeliveryMode::Pumped and DeliveryMode::Signalled",
            ));
        };
        self.settled(pumped.ready);
        pumped.waited.map(|_| pumped.ready).map_err(io::Error::from)
    }

    /// A clone of the event the thread pool sets whenever completions may be waiting, for the
    /// consumer to wait on beside its own handles, and then to [`pump`](Self::pump) (DI-7.5). Each
    /// call gives another clone, sharing the event's one handle.
    ///
    /// The event is the instance's own and shares nothing with the ring: it is auto-reset, so a wait
    /// takes the signal, and the consumer may set or reset it, which costs it nothing but its own
    /// wakeup, since a pump drains before it waits. It is set once as the instance is built, so a
    /// first wait may return with nothing to drain.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] unless the instance was built with [`DeliveryMode::Signalled`].
    pub fn completion_signal(&self) -> io::Result<Event> {
        self.delivery
            .as_ref()
            .expect("an instance is live until it ends")
            .signal()
            .unwrap_or_else(|| {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "only an instance built with DeliveryMode::Signalled has a completion signal",
                ))
            })
    }

    /// A handle to the ring's own completion event, for the consumer to wait on beside its own
    /// handles, and then to [`pump`](Self::pump), with no thread-pool callback in between
    /// ([DI-D-55](../../durable-ioring/DESIGN-NOTES.md#di-d-55)). Each call makes another
    /// duplicate of the one event.
    ///
    /// The handle carries `SYNCHRONIZE` access alone, so it can wait and nothing else: the kernel
    /// refuses a set or a reset of the ring's event through it. A holder that duplicates it asking
    /// for more access can still get it, which Windows allows, so this guards against a handle used
    /// for what it was not handed out for and is not a boundary within the process.
    ///
    /// **It is the ring's event, not a copy of it.** The event is auto-reset, so a wait through this
    /// handle takes the signal, and the pump's own wait does not see it; the pump drains before it
    /// waits, so a consumer that waits here and then pumps on the same thread loses nothing. A
    /// second thread blocked in [`pump`](Self::pump) with no limit while another waits here can be
    /// left waiting for a signal the other took, so wait and pump from one thread. The ring signals
    /// the event once as it is attached, so a first wait may return with nothing to drain.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] unless the instance was built with [`DeliveryMode::Pumped`]:
    /// the other modes give the event to a wake of their own, which a wait of the consumer's would
    /// take wakes from ([`completion_signal`](Self::completion_signal) is the event a signalled
    /// instance offers). Otherwise the duplication's: the process cannot get another handle.
    pub fn completion_event(&self) -> io::Result<OwnedHandle> {
        self.delivery
            .as_ref()
            .expect("an instance is live until it ends")
            .completion_event()
            .unwrap_or_else(|| {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "only an instance built with DeliveryMode::Pumped offers the ring's own \
                     completion event; a signalled one offers completion_signal",
                ))
            })
    }

    /// A consumer's push whose doorbell failed: set the readiness event, once, so the consumer pops,
    /// which rings it again and reports a cause that stays (DI-7+.2). With the engine's lock
    /// released.
    fn attend(&self, attention: bool) {
        if attention {
            self.hub.signal();
        }
    }

    /// After a consumer call, with the engine's lock released: set the readiness event if the call
    /// made entries poppable, then make the provider's requests.
    fn settled(&mut self, deliver: bool) {
        if deliver {
            self.hub.signal();
        }
        self.call_provider();
    }

    /// Make every request for the consumer's provider made so far, in order, with the engine's lock
    /// released: the provider may answer inside its call, and the answer takes the lock
    /// (DI-D-44). Every request is built before the first is made, so a panic in the provider drops
    /// the completions of those after it unanswered: each a failure of its domain, as DI-D-27 says.
    fn call_provider(&mut self) {
        let requests = self.engine().take_requests();
        if requests.is_empty() {
            return;
        }
        let home = answers(&self.hub);
        let requests: Vec<FlushRequest<WindowsIds<E>>> = requests
            .into_iter()
            .map(|request| flush_request(request, &home))
            .collect();
        let provider = self
            .provider
            .as_mut()
            .expect("only a provider's domains make requests");
        for request in requests {
            provider.make_durable(request);
        }
    }
}

impl<B: IoBuf, E: EpochId + 'static, C, R: IoBufMut, K: StampClock> Dioring<B, E, C, R, K> {
    /// The engine, locked: what a consumer call acts through.
    pub(crate) fn engine(&self) -> EngineGuard<'_, PoolEngine<E, B, C, K, R>> {
        self.delivery
            .as_ref()
            .expect("an instance is live until it ends")
            .engine()
    }

    /// Whether `handle` is one this instance minted. The lineage a handle names and its instance
    /// are fields anyone can fill in, through [`LineageHandle::detached`], so a fabricated handle
    /// for a real lineage would hold a reference count of its own and let an end or a retire go
    /// ahead while the genuine handles are live; the home a handle reports to is the instance's
    /// own, and is the proof.
    fn owns(&self, handle: &LineageHandle) -> bool {
        handle.is_homed_at(&self.hub)
    }

    /// End the instance, as the module's "Ending" says, returning what is left; `None` if it has
    /// ended already.
    pub(crate) fn end(&mut self) -> Option<Leftovers<DioringIds<E, K::Instant>, B, C>> {
        let delivery = self.delivery.take()?;
        // Stops the wake, waiting for a drain in progress, and takes the engine out of its slot.
        let engine = delivery.into_engine();
        // Before the wait: a completion the provider keeps in itself is answered, as abandoned, by
        // the provider's drop, which would otherwise wait for the wait it holds up.
        self.provider = None;
        self.hub.wait_for_answers();
        let Engine { core, backend, .. } = engine;
        let leftovers = core.close();
        // The ring runs down every operation still in flight as it goes.
        drop(backend);
        Some(leftovers)
    }
}

/// A request in the core's terms, as the provider is handed it: each domain's part, with the
/// completion that answers it through `home`.
fn flush_request<E: EpochId + 'static>(
    request: ProviderRequest<WindowsIds<E>>,
    home: &Weak<dyn Answer>,
) -> FlushRequest<WindowsIds<E>> {
    let ProviderRequest {
        lineage,
        seal,
        domains,
    } = request;
    let domains = domains
        .into_iter()
        .map(|ProviderPart { domain, targets }| DomainRequest {
            completion: DomainCompletion::new(
                Weak::clone(home),
                DomainCall {
                    lineage,
                    seal,
                    name: domain.clone(),
                },
            ),
            domain,
            files: targets,
        })
        .collect();
    FlushRequest { domains }
}

/// A span of the registered buffers, as the backend names it.
fn region(span: RegisteredSpan) -> RegisteredRegion {
    RegisteredRegion {
        index: span.buffer_index,
        offset: span.offset,
        length: span.len,
    }
}

/// The fault seam (`DI-3.2.4`): failures on demand, for tests and for the contract's worked
/// examples. It transforms a commit's real completion through `windows-ioring-sys`' own seam, so
/// the commit still reaches the kernel and only the answer dioring sees changes -- the backend's
/// `fail_next_commit`, under the contract's older word for a commit. It is kept beside the in-memory
/// backend's fault script, as another layer and not a copy of it (DI-D-75).
#[cfg(feature = "fault-injection")]
impl<B: IoBuf, E: EpochId + 'static, C, R: IoBufMut, K: StampClock> Dioring<B, E, C, R, K> {
    /// Make the next flush of `file` to complete report the Win32 error `code` -- for example
    /// `1117`, `ERROR_IO_DEVICE` -- whatever the kernel answered. Armed failures for one file are
    /// used in the order they were armed. The flush may already be in flight.
    pub fn fail_next_flush(&mut self, file: IoTargetKey, code: u32) {
        self.engine().backend.fail_next_commit(file, code);
    }
}

impl<B, E, C, R, K> RegisteredBufferRing for Dioring<B, E, C, R, K>
where
    B: IoBuf + Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut + Send + 'static,
    K: TimeBase,
{
    type Registered = R;

    fn with_registered_buffer<T>(
        &mut self,
        i: u32,
        f: impl FnOnce(&[u8]) -> T,
    ) -> Result<T, IoFault> {
        self.engine()
            .backend
            .registered_buffer(i)
            .map(f)
            .map_err(windows_fault)
    }

    fn with_registered_buffer_mut<T>(
        &mut self,
        i: u32,
        f: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<T, IoFault> {
        self.engine()
            .backend
            .registered_buffer_mut(i)
            .map(f)
            .map_err(windows_fault)
    }

    fn write_registered_with<T: WriteKey>(
        &mut self,
        file: T,
        offset: u64,
        span: RegisteredSpan,
        tag: Tag<'_, Self::Ids>,
        context: C,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self> {
        self.engine().write(
            file.untyped(),
            offset,
            SubmissionData::Registered(region(span)),
            tag.epoch(),
            context,
            options,
        )
    }

    fn read_registered_with<T: ReadKey>(
        &mut self,
        file: T,
        offset: u64,
        span: RegisteredSpan,
        context: C,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self> {
        self.engine().read(
            file.untyped(),
            offset,
            SubmissionData::Registered(region(span)),
            context,
            options,
        )
    }
}

impl<B: IoBuf, E: EpochId + 'static, C, R: IoBufMut, K: StampClock> Drop
    for Dioring<B, E, C, R, K>
{
    fn drop(&mut self) {
        // Ends the instance on this thread, never inside one of delivery's own calls, where
        // stopping delivery would wait for itself. Nothing to do after `close()`, which has ended
        // it already; drop discards what it would have returned.
        let _ = self.end();
    }
}
/// Register `files` with `ring`, in order, and wait for the registration to complete. `None` for
/// no files: an instance whose files are all added later makes no file registration.
///
/// Waits `bound` for the completion. Each way it fails ends construction as
/// `SetupRefusal::Ring`, and each is reached by a test (DI-7.2) through the ring crate's safe
/// responder, registered on the ring as it is created: a build refused, a submit failed, a
/// completion failed, and a completion that does not arrive within `bound`. Tried on the
/// development machine, the kernel completed a registration of a thread handle, and of a
/// zero-length buffer, successfully, so no real input reaches them; a ring without the registration
/// op (an older Windows) takes the refused build's path, and more than `u32::MAX` files takes it
/// too, refused before submission.
pub(crate) fn register_files<B, S>(
    ring: &mut IoRing<B, S>,
    handles: Vec<SharedHandle>,
    bound: Duration,
) -> io::Result<Option<RegisteredFiles>> {
    if handles.is_empty() {
        return Ok(None);
    }
    // The caller's clones: the ring keeps them for its life, and the caller keeps its own.
    let mut batch = Batch::new(ring);
    let pending = batch
        .register_shared_files(handles)
        .map_err(PushRefused::into_error)?;
    batch.submit()?;
    let completion = registration_completion(ring, bound)?;
    match pending.claim_if(&completion) {
        Ok(result) => result.map(Some),
        // Unreachable in practice: nothing else is in flight on a ring this function created a
        // moment ago, so the one completion is the registration's.
        Err(_) => Err(io::Error::other(
            "the registration's completion named another operation",
        )),
    }
}

/// A buffer registration's outcome: the registration, or `None` for no buffers; or the error with
/// the buffers, while the kernel has not been given them, and `None` once it has.
pub(crate) type BufferRegistration<R> =
    Result<Option<RegisteredBuffers<R>>, (io::Error, Option<Vec<R>>)>;

/// Register `buffers` with `ring`, and wait `bound` for the registration to complete. Its error
/// paths are reached as [`register_files`]' are; a refused build hands the buffers back, and a
/// failure after the kernel was given them does not.
pub(crate) fn register_buffers<B, S, R: IoBufMut>(
    ring: &mut IoRing<B, S>,
    buffers: Vec<R>,
    bound: Duration,
) -> BufferRegistration<R> {
    if buffers.is_empty() {
        return Ok(None);
    }
    let mut batch = Batch::new(ring);
    let pending = batch
        .register_buffers(buffers)
        .map_err(|refused| (refused.error, refused.payload))?;
    batch.submit().map_err(|error| (error, None))?;
    let completion = registration_completion(ring, bound).map_err(|error| (error, None))?;
    match pending.claim_if(&completion) {
        Ok(result) => result.map(Some).map_err(|error| (error, None)),
        // Unreachable in practice, as for files.
        Err(_) => Err((
            io::Error::other("the registration's completion named another operation"),
            None,
        )),
    }
}

/// The completion of the one registration in flight on `ring`, within `bound`.
fn registration_completion<B, S>(
    ring: &mut IoRing<B, S>,
    bound: Duration,
) -> io::Result<Completion> {
    match ring.pop_within(bound)? {
        Some((completion, _)) => Ok(completion),
        None => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("the ring did not complete a registration within {bound:?}"),
        )),
    }
}
