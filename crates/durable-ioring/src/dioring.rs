// Copyright (c) 2026 Mike Grier
//! dioring: the contract's implementation over `windows-ioring-sys`.

use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Duration;

use win_shared_os_owned_handle::SharedHandle;
use windows_ioring_sys::{
    Batch, Completion, IoBufMut, IoRing, PushRefused, RegisteredBuffers, RegisteredFile,
    RegisteredFiles,
};

use crate::contract::{DurableRing, EpochId};
use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::provider::DurabilityProvider;
use crate::types::{Epoch, FileKey, FileOptions, FlushDomain, LineageInfo, OpKind};

// `pub(crate)` so other modules' tests can build an instance with the helpers here.
#[cfg(test)]
pub(crate) mod tests;

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
/// registration completion itself failed (`windows-ioring-sys` drops them then), or when no
/// completion arrived (the ring crate leaks them rather than free memory the kernel may hold).
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
#[expect(
    dead_code,
    reason = "read, and `Shared` built, once pushes and add_file exist: DI-3.2.2"
)]
pub(crate) enum FileSlot {
    /// Given at construction: the ring holds it for its life (`windows-ioring-sys` D-81).
    Registered {
        index: RegisteredFile,
        file: SharedHandle,
    },
    /// Added later: pushed through its handle, guarded per operation. The consumer's own
    /// `SharedHandle` -- the ring takes that type directly (`windows-ioring-sys` D-82), so a file
    /// reaches the ring with no duplicate and no conversion.
    Shared(SharedHandle),
}

/// A flush domain interned by this instance: a word-sized stand-in for its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DomainId(u32);

/// One of the consumer's files, with its declared flush domains, sorted and without repeats. An
/// empty set means unknown, which intersects every file.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "read once pushes exist, from DI-3.2.2")
)]
pub(crate) struct FileRecord {
    pub(crate) target: FileSlot,
    pub(crate) domains: Box<[DomainId]>,
}

/// The consumer's provider, with the domains it serves, interned and sorted.
#[expect(
    dead_code,
    reason = "the provider is called once seals exist: DI-3.2.6"
)]
pub(crate) struct ProviderSlot<E: EpochId + 'static> {
    pub(crate) provider: Box<dyn DurabilityProvider<DioringIds<E>>>,
    pub(crate) domains: Box<[DomainId]>,
}

/// dioring's record of each operation, carried as the ring crate's sidecar. The consumer's
/// context rides inside it, so routing a completion needs no side table.
#[expect(dead_code, reason = "constructed once pushes exist, from DI-3.2.2")]
pub(crate) enum Sidecar<E: EpochId + 'static, C> {
    Consumer {
        id: OpId,
        kind: OpKind<DioringIds<E>>,
        file: FileKey,
        offset: u64,
        context: C,
    },
    Commit {
        through: Epoch<DioringIds<E>>,
        file: FileKey,
    },
}

/// dioring: `B` is the owned buffer type, `E` the epoch-id type, `C` the consumer's
/// per-operation context, and `R` the registered-buffer type.
#[expect(
    dead_code,
    reason = "the ring and its records are read once pushes exist: DI-3.2.2"
)]
pub struct Dioring<B, E: EpochId + 'static = u64, C = (), R: IoBufMut = Vec<u8>> {
    pub(crate) instance: InstanceId,
    /// Declared before `registered`, so the ring closes before the buffers it registered are
    /// released.
    pub(crate) ring: IoRing<B, Sidecar<E, C>>,
    pub(crate) files: HashMap<FileKey, FileRecord>,
    /// The interning table. An entry lives for the instance's life: the number of distinct
    /// domains is the number of devices and shares the consumer touches.
    pub(crate) domains: HashMap<FlushDomain, DomainId>,
    pub(crate) registered: Option<RegisteredBuffers<R>>,
    pub(crate) provider: Option<ProviderSlot<E>>,
}

impl<B, E: EpochId + 'static, C, R: IoBufMut> Dioring<B, E, C, R> {
    /// Build an instance and make the ring's registrations. Blocks until both have completed.
    ///
    /// # Errors
    ///
    /// A [`SetupError`] handing back what it was given, for a duplicate file key, a provider
    /// domain named twice, or a ring that could not be created or could not make a
    /// registration. The checks that need no ring run first, so a refusal for either of the
    /// first two creates nothing.
    pub fn new(setup: Setup<E, R>) -> Result<Self, SetupError<E, R>> {
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
                },
            );
        }
        let provider = provider.map(|provider| ProviderSlot {
            provider,
            domains: intern_all(&mut domains, provider_domains),
        });

        Ok(Self {
            instance: InstanceId::next(),
            ring,
            files: records,
            domains,
            registered,
            provider,
        })
    }
}

impl<B, E: EpochId + 'static, C, R: IoBufMut> DurableRing for Dioring<B, E, C, R> {
    type Ids = DioringIds<E>;
    type Buffer = B;
    type Context = C;

    fn default_lineage(&self) -> Lineage {
        Lineage {
            instance: self.instance,
            seq: 0,
        }
    }

    fn lineages(&self) -> Vec<LineageInfo<Self::Ids>> {
        vec![LineageInfo {
            lineage: self.default_lineage(),
            description: None,
            is_default: true,
            durable_through: None,
            sealed_through: None,
        }]
    }
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
) -> Box<[DomainId]> {
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
    ids.into_boxed_slice()
}
