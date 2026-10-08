// Copyright (c) 2026 Mike Grier
//! dioring: the contract's implementation over `windows-ioring-sys`.

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::{Arc, Weak};
use std::time::Duration;

use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::Event;
use win_time_sys::{Clock, InterruptClock, InterruptTime, Steady};
use windows_ioring_sys::{
    Batch, Completion, EventDelivery, IoBuf, IoBufMut, IoRing, PushRefused, RegisteredBuffers,
    RegisteredFile, RegisteredFiles, RegisteredSpan,
};

use crate::contract::{DurableRing, EntryOf, EpochId, PushResult, RegisteredBufferRing};
use crate::ids::{
    DioringIds, FailureId, FailureToken, Home, InstanceId, Lineage, LineageHandle, OpId,
};
use crate::provider::DurabilityProvider;
use crate::types::{
    AddFileError, Cause, DurabilityRequest, EndLineageError, EndLineageRefusal, Epoch, EpochState,
    FailureInfo, FileKey, FileOptions, FlushDomain, ImportScope, LineageInfo, OpKind, ReadOptions,
    Resolution, ResolveError, RetireLineageError, RetireLineageRefusal, Tag, UnknownLineage,
    WriteOptions,
};

mod durability;
mod push;
mod relay;

use durability::{DEFAULT_LINEAGE, Due, Reach, Routing, Sealing, State};
use relay::{Delivery, FlushTarget, Relay};

// `pub(crate)` so other modules' tests can build an instance with the helpers here.
#[cfg(test)]
pub(crate) mod tests;

/// A clock dioring can stamp with: one on interrupt time, its one time base (DI-D-37), whose readings
/// never decrease (`Steady`), and that can be held under the lock pool threads take (DI-D-38).
/// [`InterruptClock`] is the default; a test may give an instance its own mock, which takes on
/// `Steady`'s promise by implementing it. Implemented for every type that meets those bounds.
pub trait TimeBase: Clock<Timeline = InterruptTime> + Steady + Send + 'static {}

impl<K: Clock<Timeline = InterruptTime> + Steady + Send + 'static> TimeBase for K {}

/// How long construction waits for each registration's completion. A registration is a
/// bookkeeping operation the kernel completes at once; the bound exists so a ring that never
/// answers fails construction instead of hanging it.
const REGISTRATION_BOUND: Duration = Duration::from_secs(30);

/// What a dioring instance is built with. Both registrations are the ring's only ones (one of
/// each per ring), so they are made here or never.
pub struct Setup<E: EpochId + 'static, R> {
    /// The ring's submission queue size, as `windows-ioring-sys` takes it.
    pub submission_queue_size: u32,
    /// The ring's completion queue size, as `windows-ioring-sys` takes it.
    pub completion_queue_size: u32,
    /// Registered with the ring, which holds them for its life. A file added later is not
    /// registered.
    pub files: Vec<FileSetup>,
    /// Already allocated and placed by the consumer; dioring neither allocates nor places them.
    /// Empty registers nothing.
    pub buffers: Vec<R>,
    /// The consumer's durability provider, if any. `None` leaves every domain to the built-in
    /// default.
    pub provider: Option<Box<dyn DurabilityProvider<DioringIds<E>>>>,
}

/// One file given at construction.
#[derive(Debug)]
pub struct FileSetup {
    /// The consumer's key for it, unique within the instance.
    pub key: FileKey,
    /// The file.
    pub file: SharedHandle,
    /// Its flush domains.
    pub options: FileOptions,
}

/// Why construction failed.
#[derive(Debug)]
pub enum SetupRefusal {
    /// Two files share a key.
    DuplicateKey(FileKey),
    /// The provider named a domain twice.
    DuplicateProviderDomain(FlushDomain),
    /// Creating the ring or making a registration failed.
    Ring(io::Error),
}

/// A failed construction hands back what it was given. `buffers` is `None` only once the buffers
/// have been handed to the kernel and could not be shown to have come back: when the kernel's
/// registration completion itself failed (`windows-ioring-sys` drops them then), when no
/// completion arrived (the ring crate leaks them rather than free memory the kernel may hold), or
/// when the registration succeeded and wiring the ring's delivery then failed (the registration
/// holds them, and has no way to give them back).
///
/// That last failure is not reached by this crate's tests: `EventDelivery::new` fails where the
/// system does not support a ring completion event, or the thread pool cannot create a wait.
pub struct SetupError<E: EpochId + 'static, R> {
    /// Why construction failed.
    pub reason: SetupRefusal,
    /// The files, in the order given.
    pub files: Vec<FileSetup>,
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

/// How dioring addresses one of the consumer's files.
pub(crate) enum FileSlot {
    /// Given at construction: the ring holds it for its life (`windows-ioring-sys` D-81).
    Registered {
        index: RegisteredFile,
        #[expect(dead_code, reason = "handed back by remove_file: DI-3.2.7")]
        file: SharedHandle,
    },
    /// Added later: pushed through its handle, guarded per operation. The consumer's own
    /// `SharedHandle` -- the ring takes that type directly (`windows-ioring-sys` D-82), so a file
    /// reaches the ring with no duplicate and no conversion.
    Shared(SharedHandle),
}

impl FileSlot {
    /// What a flush of this file is pushed against.
    fn flush_target(&self) -> FlushTarget {
        match self {
            FileSlot::Registered { index, .. } => FlushTarget::Registered(*index),
            FileSlot::Shared(handle) => FlushTarget::Shared(handle.clone()),
        }
    }
}

/// A flush domain interned by this instance: a word-sized stand-in for its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DomainId(u32);

/// One of the consumer's files, with its declared flush domains, sorted and without repeats. An
/// empty set means unknown, which intersects every file.
pub(crate) struct FileRecord {
    pub(crate) target: FileSlot,
    /// Shared with the record of each write to the file, which a failure's reach is judged by.
    pub(crate) domains: Arc<[DomainId]>,
    /// Who answers for its writes, from its domains and the provider's (DI-D-27).
    pub(crate) routing: Routing,
}

/// Who answers for a file's writes (DI-D-27): the built-in default for a file with no declared
/// domain or one the provider does not serve, and the provider for the domains it serves.
fn routing(file: &[DomainId], served: Option<&ProviderSlot<impl EpochId>>) -> Routing {
    let served: &[DomainId] = served.map_or(&[], |provider| &provider.domains);
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

/// The consumer's provider, with the domains it serves, interned and sorted.
pub(crate) struct ProviderSlot<E: EpochId + 'static> {
    #[expect(dead_code, reason = "the provider is called from DI-3.2.6")]
    pub(crate) provider: Box<dyn DurabilityProvider<DioringIds<E>>>,
    pub(crate) domains: Arc<[DomainId]>,
}

/// dioring's record of each operation, carried as the ring crate's sidecar. The consumer's
/// context rides inside it, so routing a completion needs no side table.
pub(crate) enum Sidecar<E: EpochId + 'static, C> {
    Consumer {
        id: OpId,
        kind: OpKind<DioringIds<E>>,
        #[expect(dead_code, reason = "carried for the delay events: DI-3.6")]
        file: FileKey,
        #[expect(dead_code, reason = "carried for the delay events: DI-3.6")]
        offset: u64,
        context: C,
    },
    /// A seal's flush of one file (DI-D-22).
    Commit {
        through: Epoch<DioringIds<E>>,
        file: FileKey,
    },
}

/// dioring: `B` is the owned buffer type, `E` the epoch-id type, `C` the consumer's
/// per-operation context, `R` the registered-buffer type, and `K` the clock it stamps failures with
/// (a [`TimeBase`]; [`InterruptClock`] unless built with [`Dioring::with_clock`]).
///
/// The ring is handed to `EventDelivery`, whose callbacks move each completion into dioring's own
/// queue and set the readiness event, and the consumer pops that queue (DI-D-18, DI-D-28). So
/// the buffer, epoch-id and context types cross to pool threads and must be `Send`; the epoch-id
/// type must also be `Sync`, because a failure's suspect set is shared.
pub struct Dioring<B, E: EpochId + 'static = u64, C = (), R: IoBufMut = Vec<u8>, K = InterruptClock>
{
    pub(crate) instance: InstanceId,
    /// The ring, inside its delivery. Declared before `registered`: dropping it quiesces the
    /// delivery callbacks and then closes the ring, so both happen before the buffers the ring
    /// registered are released (DI-2.7 point 5, the ring crate's D-13 order).
    pub(crate) delivery: Arc<Delivery<E, B, C>>,
    pub(crate) relay: Arc<Relay<E, B, C, K>>,
    pub(crate) files: HashMap<FileKey, FileRecord>,
    /// The interning table. An entry lives for the instance's life: the number of distinct
    /// domains is the number of devices and shares the consumer touches.
    pub(crate) domains: HashMap<FlushDomain, DomainId>,
    pub(crate) registered: Option<RegisteredBuffers<R>>,
    pub(crate) provider: Option<ProviderSlot<E>>,
    /// The next operation's sequence number.
    pub(crate) next_op: u64,
    /// A copy of the default lineage's handle, so the default lineage lives as long as the
    /// instance does (DI-D-40).
    pub(crate) default: LineageHandle,
}

impl<B, E, C, R> Dioring<B, E, C, R>
where
    B: Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut,
{
    /// Build an instance, make the ring's registrations, and wire its delivery. Blocks until both
    /// registrations have completed. Returns the instance with a handle to its default lineage
    /// (DI-D-40). Failures are stamped with [`InterruptClock`]; see [`Dioring::with_clock`] for
    /// another clock.
    ///
    /// # Errors
    ///
    /// A [`SetupError`] handing back what it was given, for a duplicate file key, a provider
    /// domain named twice, or a ring that could not be created, make a registration, or be wired
    /// to deliver its completions. The checks that need no ring run first, so a refusal for
    /// either of the first two creates nothing.
    pub fn new(setup: Setup<E, R>) -> Result<(Self, LineageHandle), SetupError<E, R>> {
        Self::with_clock(setup, InterruptClock)
    }
}

impl<B, E, C, R, K> Dioring<B, E, C, R, K>
where
    B: Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut,
    K: TimeBase,
{
    /// [`Dioring::new`], stamping failures with `clock` (DI-D-38): a test's mock, for one, which
    /// must keep [`Steady`]'s promise that its readings never decrease.
    ///
    /// # Errors
    ///
    /// As [`Dioring::new`].
    pub fn with_clock(
        setup: Setup<E, R>,
        clock: K,
    ) -> Result<(Self, LineageHandle), SetupError<E, R>> {
        let Setup {
            submission_queue_size,
            completion_queue_size,
            files,
            buffers,
            provider,
        } = setup;

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

        let instance = InstanceId::next();
        let relay = match Relay::new(instance, clock) {
            Ok(relay) => Arc::new(relay),
            Err(error) => {
                return Err(SetupError {
                    reason: SetupRefusal::Ring(error),
                    files,
                    buffers: Some(buffers),
                    provider,
                });
            }
        };
        let mut ring = match IoRing::<B, Sidecar<E, C>>::with_inventory(
            submission_queue_size,
            completion_queue_size,
        ) {
            Ok(ring) => ring,
            Err(error) => {
                return Err(SetupError {
                    reason: SetupRefusal::Ring(error),
                    files,
                    buffers: Some(buffers),
                    provider,
                });
            }
        };
        let registered_files = match register_files(&mut ring, &files) {
            Ok(registered) => registered,
            Err(error) => {
                return Err(SetupError {
                    reason: SetupRefusal::Ring(error),
                    files,
                    buffers: Some(buffers),
                    provider,
                });
            }
        };
        let registered = match register_buffers(&mut ring, buffers) {
            Ok(registered) => registered,
            Err((error, buffers)) => {
                return Err(SetupError {
                    reason: SetupRefusal::Ring(error),
                    files,
                    buffers,
                    provider,
                });
            }
        };

        // After the registrations, which claim their completions from the ring directly: once
        // the ring is handed over, only the delivery callback pops it.
        let recorder = Arc::clone(&relay);
        let delivery = match EventDelivery::new(
            ring,
            move |completion, held| recorder.record(completion, held),
            None,
        ) {
            Ok(delivery) => Arc::new(delivery),
            Err(error) => {
                return Err(SetupError {
                    reason: SetupRefusal::Ring(error),
                    files,
                    buffers: None,
                    provider,
                });
            }
        };
        relay.attach(&delivery);

        let mut domains = HashMap::new();
        let mut records = HashMap::with_capacity(files.len());
        for (i, FileSetup { key, file, options }) in files.into_iter().enumerate() {
            let index = registered_files
                .and_then(|r| r.get(u32::try_from(i).ok()?))
                .expect("every file given at construction was registered, in order");
            let interned = intern_all(&mut domains, options.domains);
            records.insert(
                key,
                FileRecord {
                    target: FileSlot::Registered { index, file },
                    domains: interned,
                    routing: Routing {
                        default: true,
                        provider: false,
                    },
                },
            );
        }
        let provider = provider.map(|provider| ProviderSlot {
            provider,
            domains: intern_all(&mut domains, provider_domains),
        });
        for record in records.values_mut() {
            record.routing = routing(&record.domains, provider.as_ref());
        }

        let default = LineageHandle::mint(
            Lineage {
                instance,
                seq: DEFAULT_LINEAGE,
            },
            home(&relay),
        );
        let handle = default.clone();
        Ok((
            Self {
                instance,
                delivery,
                relay,
                files: records,
                domains,
                registered,
                provider,
                next_op: 0,
                default,
            },
            handle,
        ))
    }
}

/// Where an instance's lineage handles report their release.
fn home<T: Home + 'static>(relay: &Arc<T>) -> Weak<dyn Home> {
    let home: Weak<T> = Arc::downgrade(relay);
    home
}

impl<B, E, C, R, K> DurableRing for Dioring<B, E, C, R, K>
where
    B: Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut,
    K: TimeBase,
{
    type Ids = DioringIds<E>;
    type Buffer = B;
    type Context = C;

    fn readiness(&mut self) -> io::Result<Event> {
        self.relay.readiness()
    }

    fn add_file_with(
        &mut self,
        key: FileKey,
        file: SharedHandle,
        options: FileOptions,
    ) -> Result<(), AddFileError> {
        if self.files.contains_key(&key) {
            return Err(AddFileError { key, file, options });
        }
        let domains = intern_all(&mut self.domains, options.domains);
        let routing = routing(&domains, self.provider.as_ref());
        self.files.insert(
            key,
            FileRecord {
                target: FileSlot::Shared(file),
                domains,
                routing,
            },
        );
        Ok(())
    }

    fn write_with(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: B,
        tag: Tag<'_, Self::Ids>,
        context: C,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        B: IoBuf,
    {
        self.push_write(file, offset, buffer, tag, context, options)
    }

    fn read_with(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: B,
        context: C,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        B: IoBufMut,
    {
        self.push_read(file, offset, buffer, context, options)
    }

    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>> {
        // A failed submission, and a flush the ring refused, are retried here as well as at the
        // next change, so a consumer that stops pushing still gets them issued. A failed retry is
        // reported only when there is no entry to return instead: an entry is progress, and the
        // retry runs again at the next pop.
        let mut core = self.relay.lock();
        // Recorded on whichever thread observed it, usually a pool thread; asserted here, where a
        // panic fails the consumer's call rather than aborting the process.
        debug_assert!(
            core.durability.inconsistency().is_none(),
            "dioring's durability state is inconsistent: {:?}",
            core.durability.inconsistency()
        );
        let retried = self.relay.apply(&mut core, Due::default(), &self.delivery);
        match (Relay::pop(&mut core), retried) {
            (Some(entry), _) => Ok(Some(entry)),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(None),
        }
    }

    fn make_durable_through(
        &mut self,
        through: Tag<'_, Self::Ids>,
    ) -> io::Result<DurabilityRequest<Self::Ids>> {
        let through = through.epoch();
        if !self.is_live(through.lineage) {
            return Err(foreign_lineage());
        }
        let mut core = self.relay.lock();
        match core.durability.seal(through.lineage.seq, through.id) {
            Sealing::AlreadySealed(state) => {
                Ok(DurabilityRequest::AlreadySealed(epoch_state(state)))
            }
            Sealing::Submitted(due) => {
                self.relay.apply(&mut core, due, &self.delivery);
                Ok(DurabilityRequest::Submitted)
            }
        }
    }

    fn durable_through(&self, lineage: Lineage) -> Result<Option<E>, UnknownLineage<Self::Ids>> {
        self.ours(lineage)?;
        (self.relay.lock().durability)
            .durable_through(lineage.seq)
            .ok_or(UnknownLineage(lineage))
    }

    fn sealed_through(&self, lineage: Lineage) -> Result<Option<E>, UnknownLineage<Self::Ids>> {
        self.ours(lineage)?;
        (self.relay.lock().durability)
            .sealed_through(lineage.seq)
            .ok_or(UnknownLineage(lineage))
    }

    fn epoch_state(
        &self,
        epoch: Epoch<Self::Ids>,
    ) -> Result<EpochState<Self::Ids>, UnknownLineage<Self::Ids>> {
        self.ours(epoch.lineage)?;
        (self.relay.lock().durability)
            .state(epoch.lineage.seq, epoch.id)
            .map(epoch_state)
            .ok_or(UnknownLineage(epoch.lineage))
    }

    fn default_lineage(&self) -> LineageHandle {
        self.default.clone()
    }

    fn mint_lineage(&mut self, description: Option<String>) -> LineageHandle {
        let seq = self.relay.lock().durability.mint(description);
        LineageHandle::mint(
            Lineage {
                instance: self.instance,
                seq,
            },
            home(&self.relay),
        )
    }

    fn lineages(&self) -> Vec<LineageInfo<Self::Ids>> {
        self.relay.lock().durability.lineages()
    }

    fn end_lineage(&mut self, handle: LineageHandle) -> Result<(), EndLineageError<Self::Ids>> {
        let lineage = handle.lineage();
        let reason = if lineage.instance != self.instance {
            EndLineageRefusal::Foreign(lineage)
        } else if lineage.seq == DEFAULT_LINEAGE {
            EndLineageRefusal::Default
        } else {
            match handle.take_last() {
                Ok(lineage) => {
                    let mut core = self.relay.lock();
                    let due = core.durability.end(lineage.seq);
                    self.relay.apply(&mut core, due, &self.delivery);
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
        let reason = if lineage.instance != self.instance {
            RetireLineageRefusal::Foreign(lineage)
        } else if lineage.seq == DEFAULT_LINEAGE {
            RetireLineageRefusal::Default
        } else if !handle.is_last() {
            RetireLineageRefusal::Shared
        } else {
            // Unlocked before the handle is consumed: a handle's release takes this lock.
            let retired = self.relay.lock().durability.retire(lineage.seq);
            match retired {
                Err(busy) => RetireLineageRefusal::Busy(busy),
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
        let mut core = self.relay.lock();
        let due = core.durability.resolve(items)?;
        self.relay.apply(&mut core, due, &self.delivery);
        Ok(())
    }

    fn import_failure(
        &mut self,
        scope: ImportScope<Self::Ids>,
    ) -> Result<FailureId, UnknownLineage<Self::Ids>> {
        let (reach, only) = self.reach(&scope)?;
        let mut core = self.relay.lock();
        let (id, due) = core
            .durability
            .import(Cause::Imported { scope }, &reach, only);
        self.relay.apply(&mut core, due, &self.delivery);
        Ok(id)
    }

    fn failures(&self) -> Vec<FailureInfo<Self::Ids>> {
        self.relay.lock().durability.failures()
    }

    fn take_token(&mut self, failure: FailureId) -> Option<FailureToken> {
        self.relay.lock().durability.take_token(failure)
    }
}

impl<B, E, C, R, K> Dioring<B, E, C, R, K>
where
    B: Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut,
    K: TimeBase,
{
    /// The files an imported failure reaches (DI-D-21), and the one lineage it is confined to, if
    /// the scope names one. A domain no file was declared with reaches only the files declared
    /// with none.
    fn reach(
        &self,
        scope: &ImportScope<DioringIds<E>>,
    ) -> Result<(Reach, Option<u64>), UnknownLineage<DioringIds<E>>> {
        Ok(match scope {
            ImportScope::All => (Reach::All, None),
            ImportScope::Lineage(lineage) if self.is_live(*lineage) => {
                (Reach::All, Some(lineage.seq))
            }
            ImportScope::Lineage(lineage) => return Err(UnknownLineage(*lineage)),
            ImportScope::Domains(domains) => (
                Reach::Domains(
                    domains
                        .iter()
                        .filter_map(|domain| self.domains.get(domain).copied())
                        .collect(),
                ),
                None,
            ),
        })
    }

    /// Refuse another instance's lineage, which this one has no answer for.
    fn ours(&self, lineage: Lineage) -> Result<(), UnknownLineage<DioringIds<E>>> {
        if lineage.instance == self.instance {
            Ok(())
        } else {
            Err(UnknownLineage(lineage))
        }
    }
}

/// The fault seam (`DI-3.2.4`): failures on demand, for tests and for the contract's worked
/// examples. It transforms a flush's real completion through `windows-ioring-sys`' own seam, so
/// the flush still reaches the kernel and only the answer dioring sees changes.
#[cfg(feature = "fault-injection")]
impl<B, E: EpochId + 'static, C, R: IoBufMut, K> Dioring<B, E, C, R, K> {
    /// Make the next flush of `file` to complete report the Win32 error `code` -- for example
    /// `1117`, `ERROR_IO_DEVICE` -- whatever the kernel answered. Armed failures for one file are
    /// used in the order they were armed. The flush may already be in flight.
    pub fn fail_next_flush(&mut self, file: FileKey, code: u32) {
        self.relay.lock().injected.push((file, code));
    }
}

impl<B, E, C, R, K> RegisteredBufferRing for Dioring<B, E, C, R, K>
where
    B: Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut,
    K: TimeBase,
{
    type Registered = R;

    fn registered_buffer(&mut self, i: u32) -> io::Result<&[u8]> {
        self.registration()?.get(i)
    }

    fn registered_buffer_mut(&mut self, i: u32) -> io::Result<&mut [u8]> {
        self.registration()?.get_mut(i)
    }

    fn write_registered_with(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        tag: Tag<'_, Self::Ids>,
        context: C,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self> {
        self.push_write_registered(file, offset, span, tag, context, options)
    }

    fn read_registered_with(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        context: C,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self> {
        self.push_read_registered(file, offset, span, context, options)
    }
}

impl<B, E: EpochId + 'static, C, R: IoBufMut, K> Drop for Dioring<B, E, C, R, K> {
    fn drop(&mut self) {
        // Before the delivery is dropped, which the field drop that follows does: once the core is
        // closed no callback holds the delivery, so it is dropped here, on this thread, and not
        // inside one of its own callbacks.
        self.relay.close();
    }
}

/// The contract's spelling of a lineage's epoch state.
fn epoch_state<E: EpochId + 'static>(state: State) -> EpochState<DioringIds<E>> {
    match state {
        State::Open => EpochState::Open,
        State::Pending => EpochState::Pending,
        State::Blocked(by) => EpochState::Blocked(by),
        State::Durable => EpochState::Durable,
        State::Abandoned => EpochState::Abandoned,
    }
}

/// A seal request naming a lineage this instance does not have.
fn foreign_lineage() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "the epoch's lineage is not one of this instance's",
    )
}

/// Register `files` with `ring`, in order, and wait for the registration to complete. `None` for
/// no files: an instance whose files are all added later makes no file registration.
///
/// The error paths here are not reached by this crate's tests. Tried on the development machine,
/// the kernel completed a registration of a thread handle, and of a zero-length buffer,
/// successfully. What would reach them is a ring without the registration op (an older Windows),
/// more than `u32::MAX` files, a completion that never arrives, or the kernel failing the
/// completion. Each ends construction as `SetupRefusal::Ring`.
fn register_files<B, S>(
    ring: &mut IoRing<B, S>,
    files: &[FileSetup],
) -> io::Result<Option<RegisteredFiles>> {
    if files.is_empty() {
        return Ok(None);
    }
    // Clones: the ring keeps its own for its life, and dioring keeps the consumer's.
    let handles = files.iter().map(|f| f.file.clone()).collect();
    let mut batch = Batch::new(ring);
    let pending = batch
        .register_shared_files(handles)
        .map_err(PushRefused::into_error)?;
    batch.submit()?;
    let completion = registration_completion(ring)?;
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
type BufferRegistration<R> = Result<Option<RegisteredBuffers<R>>, (io::Error, Option<Vec<R>>)>;

/// Register `buffers` with `ring`, and wait for the registration to complete. Its error paths are
/// unreached by tests, as [`register_files`]' are; a buffer longer than `u32::MAX` also reaches
/// one, refused before submission with the buffers handed back.
fn register_buffers<B, S, R: IoBufMut>(
    ring: &mut IoRing<B, S>,
    buffers: Vec<R>,
) -> BufferRegistration<R> {
    if buffers.is_empty() {
        return Ok(None);
    }
    let mut batch = Batch::new(ring);
    let pending = batch
        .register_buffers(buffers)
        .map_err(|refused| (refused.error, refused.payload))?;
    batch.submit().map_err(|error| (error, None))?;
    let completion = registration_completion(ring).map_err(|error| (error, None))?;
    match pending.claim_if(&completion) {
        Ok(result) => result.map(Some).map_err(|error| (error, None)),
        // Unreachable in practice, as for files.
        Err(_) => Err((
            io::Error::other("the registration's completion named another operation"),
            None,
        )),
    }
}

/// The completion of the one registration in flight on `ring`.
fn registration_completion<B, S>(ring: &mut IoRing<B, S>) -> io::Result<Completion> {
    match ring.pop_within(REGISTRATION_BOUND)? {
        Some((completion, _)) => Ok(completion),
        None => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "the ring did not complete a registration within {}s",
                REGISTRATION_BOUND.as_secs()
            ),
        )),
    }
}

/// The first value `values` yields twice.
fn first_repeat<T: Eq + std::hash::Hash + Clone>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut seen = HashSet::new();
    values.into_iter().find(|value| !seen.insert(value.clone()))
}

/// Intern `domains`, returning their ids sorted and without repeats.
fn intern_all(
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
