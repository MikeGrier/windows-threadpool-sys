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

use std::collections::{HashMap, HashSet};
use std::io;
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

use crate::backend::{IoBackend, RegisteredRegion, SubmissionData};
use crate::clock::{Instant, StampClock};
use crate::contract::{DurableRing, EntryOf, EpochId, PushResult, RegisteredBufferRing};
use crate::diagnostics::{DiagnosticSink, NoDiagnostics};
use crate::engine::{Engine, EngineGuard};
use crate::ids::{DioringIds, FailureId, FailureToken, Home, InstanceId, Lineage, LineageHandle};
use crate::io_core::DurableIoCore;
use crate::io_target::IoTarget;
use crate::provider::{
    Answer, DomainCall, DomainCompletion, DomainRequest, DurabilityProvider, FlushRequest,
    ProviderPart, ProviderRequest,
};
use crate::pump::PumpedDelivery;
use crate::types::{
    AddTargetError, DurabilityRequest, EndLineageError, EndLineageRefusal, Epoch, EpochState,
    FailureInfo, FlushDomain, ImportScope, Imported, IoTargetKey, Leftovers, LineageInfo,
    ReadOptions, RemoveTargetError, Resolution, ResolveError, RetireLineageError,
    RetireLineageRefusal, SealError, Tag, UnknownLineage, WriteOptions,
};
use crate::windows_backend::pool::{PoolDelivery, PoolEngine};
use crate::windows_backend::{
    Construction, Win32FileTarget, WindowsBackend, WindowsBackendError, WindowsBackendRefusal,
    WindowsBackendSetup,
};

mod delivery;
pub(crate) mod durability;
mod hub;

use delivery::Delivery;
use durability::{Reach, Routing, State};
use hub::Hub;

// `pub(crate)` so other modules' tests can build an instance with the helpers here.
#[cfg(test)]
pub(crate) mod tests;

/// A clock dioring can stamp with: one on interrupt time, its one time base (DI-D-37), whose readings
/// never decrease (`Steady`), and that can be held under the lock pool threads take (DI-D-38).
/// [`InterruptClock`] is the default; a test may give an instance its own mock, which takes on
/// `Steady`'s promise by implementing it. Implemented for every type that meets those bounds.
pub trait TimeBase: Clock<Timeline = InterruptTime> + Steady + Send + 'static {}

impl<K: Clock<Timeline = InterruptTime> + Steady + Send + 'static> TimeBase for K {}

/// The time type the Windows driver's stamps are read in: interrupt time, its one time base
/// (DI-D-37).
pub(crate) type WinInstant = TimePoint<InterruptTime>;

/// An interrupt-time clock is the core's stamp clock on Windows (DI-D-50): its readings never
/// decrease, which is `Steady`'s promise.
impl<K: Clock<Timeline = InterruptTime> + Steady + Send + 'static> StampClock for K {
    type Instant = WinInstant;

    fn now(&self) -> WinInstant {
        Clock::now(self)
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
    pub provider: Option<Box<dyn DurabilityProvider<DioringIds<E>>>>,
    /// Where the instance's diagnostics go (DI-D-51). `None` is the driver's default, which is
    /// [`NoDiagnostics`] until `DI-12.1` makes it ETW.
    pub diagnostics: Option<Arc<dyn DiagnosticSink>>,
    /// How the instance's completions are drained (DI-D-52, DI-D-55).
    pub delivery: DeliveryMode<'env, 'pool>,
}

/// How an instance's completions are drained: by the thread pool, or by the consumer's own thread.
/// Chosen once, as the instance is built, and not changed after.
pub enum DeliveryMode<'env, 'pool> {
    /// Drained on a pool thread whenever the ring's completion event says to, and the consumer
    /// told through [`DurableRing::readiness`]. Holds the thread pool, and the priority within it,
    /// that delivery runs on -- a NUMA node's own pool, for one; `None` is the process's default
    /// pool. Read once, as the instance is built.
    Pool(Option<&'env mut CallbackEnviron<'pool>>),
    /// Drained by the consumer's own thread, which calls [`Dioring::pump`]: no thread of the
    /// instance's own, and none of the pool's. Nothing is drained until it does.
    Pumped,
}

/// One target given at construction.
#[derive(Debug)]
pub struct TargetSetup {
    /// The consumer's key for it, unique within the instance.
    pub key: IoTargetKey,
    /// The target: its file, and what it states, flush domains among it.
    pub target: Win32FileTarget,
}

/// Why construction failed.
#[derive(Debug)]
pub enum SetupRefusal {
    /// Two targets share a key.
    DuplicateKey(IoTargetKey),
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
    pub provider: Option<Box<dyn DurabilityProvider<DioringIds<E>>>>,
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

/// A flush domain interned by this instance: a word-sized stand-in for its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DomainId(u32);

/// Who answers for a file's writes (DI-D-27): the built-in default for a file with no declared
/// domain or one the provider does not serve, and the provider for the domains it serves.
pub(crate) fn routing(file: &[DomainId], served: &[DomainId]) -> Routing {
    if file.is_empty() {
        return Routing {
            default: true,
            provider: false,
        };
    }
    Routing {
        default: file.iter().any(|domain| !served.contains(domain)),
        provider: file.iter().any(|domain| served.contains(domain)),
    }
}

/// dioring: `B` is the owned buffer type, `E` the epoch-id type, `C` the consumer's
/// per-operation context, `R` the registered-buffer type, and `K` the clock it stamps failures with
/// (a [`TimeBase`]; [`InterruptClock`] unless built with [`Dioring::with_clock`]).
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
    K: StampClock = InterruptClock,
> {
    /// The core and its backend, and how they are drained; `None` once the instance has ended.
    pub(crate) delivery: Option<Delivery<E, B, C, K, R>>,
    pub(crate) hub: Arc<Hub<E, B, C, K, R>>,
    pub(crate) provider: Option<Box<dyn DurabilityProvider<DioringIds<E>>>>,
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
    /// (DI-D-40). Failures are stamped with [`InterruptClock`]; see [`Dioring::with_clock`] for
    /// another clock.
    ///
    /// # Errors
    ///
    /// A [`SetupError`] handing back what it was given, for a duplicate target key, a provider
    /// domain named twice, or a ring that could not be created, make a registration, or be wired
    /// to deliver its completions. The checks that need no ring run first, so a refusal for
    /// either of the first two creates nothing.
    pub fn new(setup: Setup<'_, '_, E, R>) -> Result<(Self, LineageHandle), SetupError<E, R>> {
        Self::with_clock(setup, InterruptClock)
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
    /// must keep [`Steady`]'s promise that its readings never decrease.
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
        let diagnostics = diagnostics.unwrap_or_else(|| Arc::new(NoDiagnostics));

        if let Some(key) = first_repeat(files.iter().map(|f| f.key)) {
            return Err(SetupError {
                reason: SetupRefusal::DuplicateKey(key),
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

        let signals = Event::new(ResetMode::Auto, false)
            .and_then(|readiness| Ok((readiness.try_clone()?, readiness)));
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
        for file in &files {
            core.add_target(file.key, file.target.describe())
                .expect("the keys were checked unique above");
        }
        let targets = files
            .into_iter()
            .map(|TargetSetup { key, target }| (key, target))
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
                    files: setups(targets),
                    buffers,
                    provider,
                });
            }
        };

        let default_lineage = core.default_lineage();
        let on_ready = move || {
            // Setting an event this instance created, with full access, does not fail.
            let set = ready.set();
            debug_assert!(set.is_ok(), "setting the readiness event failed: {set:?}");
        };
        let engine = Engine::new(core, backend);
        let delivered = match delivery {
            DeliveryMode::Pool(environment) => {
                PoolDelivery::new(engine, on_ready, environment).map(Delivery::Pool)
            }
            // Nothing sets the signal the pool's wake would: entries are made poppable by the
            // pump, which sets the readiness event itself, as every other path does.
            DeliveryMode::Pumped => PumpedDelivery::new(engine).map(Delivery::Pumped),
        };
        let delivery = match delivered {
            Ok(delivery) => delivery,
            Err((error, engine)) => {
                let mut backend = engine.backend;
                let files = keys
                    .into_iter()
                    .filter_map(|key| {
                        backend
                            .detach(key)
                            .map(|target| TargetSetup { key, target })
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
fn setups(targets: Vec<(IoTargetKey, Win32FileTarget)>) -> Vec<TargetSetup> {
    targets
        .into_iter()
        .map(|(key, target)| TargetSetup { key, target })
        .collect()
}
/// What an import reaches: the files, the one lineage it is confined to if any, and the domains it
/// named that the instance has never been given.
pub(crate) type ImportReach = (Reach, Option<u64>, Vec<FlushDomain>);

/// The targets an imported failure reaches (DI-D-21), the one lineage it is confined to, if the
/// scope names one, and the domains it names that the instance has never been given, in the order
/// named (DI-D-46). A domain no target was declared with reaches only the targets declared with
/// none. `live` says whether a lineage is the instance's own and live; `domains` is every domain
/// the instance has interned.
pub(crate) fn import_reach<E: EpochId + 'static, I: Instant>(
    scope: &ImportScope<DioringIds<E, I>>,
    domains: &HashMap<FlushDomain, DomainId>,
    live: impl Fn(Lineage) -> bool,
) -> Result<ImportReach, UnknownLineage<DioringIds<E, I>>> {
    Ok(match scope {
        ImportScope::All => (Reach::All, None, Vec::new()),
        ImportScope::Lineage(lineage) if live(*lineage) => {
            (Reach::All, Some(lineage.seq), Vec::new())
        }
        ImportScope::Lineage(lineage) => return Err(UnknownLineage(*lineage)),
        ImportScope::Domains(names) => {
            let mut known = Vec::with_capacity(names.len());
            let mut unmatched = Vec::new();
            for domain in names {
                match domains.get(domain) {
                    Some(&id) => known.push(id),
                    None => unmatched.push(domain.clone()),
                }
            }
            (Reach::Domains(known.into()), None, unmatched)
        }
    })
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
    type Ids = DioringIds<E>;
    type Buffer = B;
    type Context = C;
    type Readiness = Event;
    type Target = Win32FileTarget;

    fn readiness(&mut self) -> io::Result<Event> {
        self.hub.readiness()
    }

    fn add_target(
        &mut self,
        key: IoTargetKey,
        target: Win32FileTarget,
    ) -> Result<(), AddTargetError<Win32FileTarget>> {
        let mut engine = self.engine();
        if engine.core.target(key).is_some() || engine.backend.target(key).is_some() {
            return Err(AddTargetError { key, target });
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

    fn write_with(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        buffer: B,
        tag: Tag<'_, Self::Ids>,
        context: C,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        B: IoBuf,
    {
        self.engine().write(
            file,
            offset,
            SubmissionData::Owned(buffer),
            tag.epoch(),
            context,
            options,
        )
    }

    fn read_with(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        buffer: B,
        context: C,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        B: IoBufMut,
    {
        let mut engine = self.engine();
        // Here, where `B: IoBufMut` holds, the backend is given the push an owned read needs
        // (DI-D-17).
        engine.backend.enable_owned_reads();
        engine.read(
            file,
            offset,
            SubmissionData::Owned(buffer),
            context,
            options,
        )
    }

    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>> {
        // A push submits as it goes, so what is left to submit here is what a failed submission
        // left queued. It is reported only when there is no entry to return instead: an entry is
        // progress, and the submission is tried again at the next pop.
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
            match (engine.core.pop(), submitted) {
                (Some(entry), _) => Ok(Some(entry)),
                (None, Err(cause)) => Err(io::Error::other(cause)),
                (None, Ok(())) => Ok(None),
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
        let reason = if lineage.instance != self.instance() {
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
        let reason = if lineage.instance != self.instance() {
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
    /// Drain what the ring has completed, on this thread (DI-D-55): wait up to `timeout` for its
    /// completion event -- `None` is no limit, and `Duration::ZERO` is a pump that does not wait --
    /// then drain it to empty, make the provider's requests, and set the readiness event if that
    /// made entries poppable. Whether it did. Pop what became poppable with [`DurableRing::pop`].
    ///
    /// **Only for an instance built with [`DeliveryMode::Pumped`]**, where nothing else drains: a
    /// completion waits in the ring until a pump, so an instance that is never pumped delivers
    /// nothing. The wait is made with the engine's lock released, so another thread's push, a
    /// provider's answer or a lineage handle's release is not held up by it.
    ///
    /// A return does not promise entries: the wait is satisfied by a raising whose completions an
    /// earlier drain took, and by the one signal the ring crate raises as it attaches the event
    /// ([D-20](../../windows-ioring-sys/DESIGN-NOTES.md#d-20)), so a first pump returns at once.
    /// Pump in a loop, and pop to empty each time. With no limit and nothing in flight a pump never
    /// returns.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] for an instance built with [`DeliveryMode::Pool`], which the
    /// pool drains. Otherwise the wait's own error, when the ring's completion event cannot be
    /// waited on; the drain is made all the same, so what has completed is delivered, and the entries
    /// it made poppable are there to pop.
    pub fn pump(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        let pumped = match self
            .delivery
            .as_mut()
            .expect("an instance is live until it ends")
        {
            Delivery::Pumped(pumped) => pumped.pump(timeout),
            Delivery::Pool(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "the instance was built with pool delivery, which drains it; pumping is for \
                     DeliveryMode::Pumped",
                ));
            }
        };
        self.settled(pumped.ready);
        pumped.waited.map(|_| pumped.ready)
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
        let requests: Vec<FlushRequest<DioringIds<E>>> = requests
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

    /// The instance, as its lineages name it.
    fn instance(&self) -> InstanceId {
        self.default.lineage().instance
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
    request: ProviderRequest<DioringIds<E>>,
    home: &Weak<dyn Answer>,
) -> FlushRequest<DioringIds<E>> {
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
/// `fail_next_commit`, under the contract's older word for a commit (DI-11.2).
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

    fn with_registered_buffer<T>(&mut self, i: u32, f: impl FnOnce(&[u8]) -> T) -> io::Result<T> {
        self.engine().backend.registered_buffer(i).map(f)
    }

    fn with_registered_buffer_mut<T>(
        &mut self,
        i: u32,
        f: impl FnOnce(&mut [u8]) -> T,
    ) -> io::Result<T> {
        self.engine().backend.registered_buffer_mut(i).map(f)
    }

    fn write_registered_with(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        span: RegisteredSpan,
        tag: Tag<'_, Self::Ids>,
        context: C,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self> {
        self.engine().write(
            file,
            offset,
            SubmissionData::Registered(region(span)),
            tag.epoch(),
            context,
            options,
        )
    }

    fn read_registered_with(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        span: RegisteredSpan,
        context: C,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self> {
        self.engine().read(
            file,
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
/// The contract's spelling of a lineage's epoch state.
pub(crate) fn epoch_state<E: EpochId + 'static, I: Instant>(
    state: State,
) -> EpochState<DioringIds<E, I>> {
    match state {
        State::Open => EpochState::Open,
        State::Pending => EpochState::Pending,
        State::Blocked(by) => EpochState::Blocked(by),
        State::Durable => EpochState::Durable,
        State::Abandoned => EpochState::Abandoned,
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

/// The first value `values` yields twice.
pub(crate) fn first_repeat<T: Eq + std::hash::Hash + Clone>(
    values: impl IntoIterator<Item = T>,
) -> Option<T> {
    let mut seen = HashSet::new();
    values.into_iter().find(|value| !seen.insert(value.clone()))
}

/// Intern `domains`, returning their ids sorted and without repeats.
pub(crate) fn intern_all(
    table: &mut HashMap<FlushDomain, DomainId>,
    domains: impl IntoIterator<Item = FlushDomain>,
) -> Arc<[DomainId]> {
    let mut ids: Vec<DomainId> = domains
        .into_iter()
        .map(|domain| {
            let next = DomainId(
                u32::try_from(table.len()).expect("fewer than 2^32 distinct flush domains"),
            );
            *table.entry(domain).or_insert(next)
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into()
}
