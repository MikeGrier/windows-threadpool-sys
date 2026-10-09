// Copyright (c) 2026 Mike Grier
//! The contract's shared values. Those that carry an identity are generic over the
//! implementation's [`Identities`]; the rest are plain.

use std::cmp::Ordering;
use std::fmt;
use std::io;
use std::sync::Arc;

use win_shared_os_owned_handle::SharedHandle;

use crate::contract::Identities;
use crate::error_code::ErrorCode;

#[cfg(test)]
mod tests;

/// The consumer's identity for a file, supplied when the file is given to dioring and used in
/// every report, so a report stays meaningful after the file's handle is gone (DI-D-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IoTargetKey(pub u64);

/// An epoch: an epoch id within a lineage, by the lineage's plain name. What the API reports, and
/// what names or asks about an epoch -- a gate, a query -- takes; what acts on a lineage takes a
/// [`Tag`] instead (DI-D-40). Plain data, so a consumer can keep it in its own records.
///
/// Only `PartialOrd`: epochs of different lineages are unrelated, so `partial_cmp` is `None`
/// for them, and within one lineage they order by id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Epoch<V: Identities> {
    /// The lineage whose epoch space the id belongs to.
    pub lineage: V::Lineage,
    /// The consumer's epoch id within that lineage.
    pub id: V::EpochId,
}

impl<V: Identities> Epoch<V> {
    /// The epoch `id` of `lineage`.
    pub fn new(lineage: V::Lineage, id: V::EpochId) -> Self {
        Self { lineage, id }
    }
}

impl<V: Identities> PartialOrd for Epoch<V> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (self.lineage == other.lineage).then(|| self.id.cmp(&other.id))
    }
}

/// What acts on an epoch of a live lineage: an epoch id, with the lineage's handle (DI-D-40). A
/// write is tagged with one, and a seal names one, so neither can name a lineage that is not live.
/// Borrows the handle, never the instance.
pub struct Tag<'h, V: Identities> {
    handle: &'h V::LineageHandle,
    id: V::EpochId,
}

impl<'h, V: Identities> Tag<'h, V> {
    /// Epoch `id` of `handle`'s lineage.
    pub fn new(handle: &'h V::LineageHandle, id: V::EpochId) -> Self {
        Self { handle, id }
    }

    /// The lineage's handle.
    pub fn handle(&self) -> &'h V::LineageHandle {
        self.handle
    }

    /// The epoch id.
    pub fn id(&self) -> V::EpochId {
        self.id
    }

    /// The epoch, by the lineage's plain name.
    pub fn epoch(&self) -> Epoch<V> {
        Epoch::new(V::handle_lineage(self.handle), self.id)
    }
}

impl<V: Identities> Clone for Tag<'_, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V: Identities> Copy for Tag<'_, V> {}

impl<V: Identities> fmt::Debug for Tag<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Tag").field(&self.epoch()).finish()
    }
}

/// A plain lineage, or an epoch of one, that the instance cannot answer for: another instance's,
/// or one ended or retired (DI-D-40). The instance keeps no memory of a lineage once it is gone,
/// as it keeps none of a resolved failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownLineage<V: Identities>(pub V::Lineage);

impl<V: Identities> fmt::Display for UnknownLineage<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a lineage this instance can answer for",
            self.0
        )
    }
}

impl<V: Identities> std::error::Error for UnknownLineage<V> {}

/// One lineage, as the enumeration reports it. Owned, so holding it borrows nothing.
#[derive(Clone, Debug)]
pub struct LineageInfo<V: Identities> {
    /// The lineage.
    pub lineage: V::Lineage,
    /// The consumer's description, if one was supplied when the lineage was minted.
    pub description: Option<Arc<str>>,
    /// The instance's default lineage, which never ends while the instance lives: the instance
    /// holds a copy of its handle.
    pub is_default: bool,
    /// The lineage's high-water mark: the highest id through which every epoch is durable or
    /// abandoned, or `None` before anything is.
    pub durable_through: Option<V::EpochId>,
    /// The lineage's seal point, or `None` before anything is sealed.
    pub sealed_through: Option<V::EpochId>,
}

/// One write a failure put at risk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuspectWrite<V: Identities> {
    /// The write's identity.
    pub op: V::OpId,
    /// The target it wrote.
    pub target: IoTargetKey,
    /// The epoch it was tagged with.
    pub epoch: Epoch<V>,
}

/// A failure's suspect set: frozen when the failure is observed, shared rather than copied. It never
/// gains a write; what happens to its writes afterwards is recorded as [`Marking`]s (DI-D-36).
#[derive(Clone, Debug)]
pub struct SuspectSet<V: Identities>(Arc<[SuspectWrite<V>]>);

impl<V: Identities> SuspectSet<V> {
    /// Freeze `writes`, in push order.
    pub(crate) fn new(writes: Vec<SuspectWrite<V>>) -> Self {
        Self(writes.into())
    }

    /// The set's writes. The one borrow the API returns, into frozen shared data that nothing
    /// can change.
    pub fn writes(&self) -> &[SuspectWrite<V>] {
        &self.0
    }
}

/// Where an imported failure lands (DI-2.10 point 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportScope<V: Identities> {
    /// Every uncovered write in the instance.
    All,
    /// The uncovered writes of one lineage.
    Lineage(V::Lineage),
    /// The uncovered writes to every file whose declared flush domains intersect these, files
    /// declared with none included.
    Domains(Vec<FlushDomain>),
}

/// What `import_failure` recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported<V: Identities> {
    /// The failure, whose token arrives with its `Failed` entry.
    pub failure: V::FailureId,
    /// The domains an [`ImportScope::Domains`] named that the instance has never been given -- by
    /// any file or by its provider -- in the order named. Each reaches only the files declared with
    /// none, which is the right answer when declarations are complete; a name here that the
    /// consumer expected to match is a misspelling, or a file not yet given. Empty for every other
    /// scope.
    pub unmatched: Vec<FlushDomain>,
}

/// A flush domain: a unit whose volatile cache a flush commits and whose failure everything
/// behind it shares -- a disk, a controller cache, a virtual disk, a remote share (DI-D-21).
/// Named by the consumer's exact identifier bytes, so two instances or processes name one
/// domain alike. dioring interns each distinct domain into a word-sized value of its own, so
/// comparisons after declaration are cheap, and reports return the bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FlushDomain(Arc<[u8]>);

impl FlushDomain {
    /// The domain named by `bytes`.
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self(bytes.into())
    }

    /// The bytes that name the domain.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// The advanced parameters of giving a file to dioring, taken by `TargetSetup` at construction.
/// Set once, when the file is given; to change them, remove the file and add it again
/// (DI-2.10 point 3).
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct FileOptions {
    pub(crate) domains: Vec<FlushDomain>,
}

impl FileOptions {
    /// No declared flush domains: the file is unknown and shares fate with every file.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare every flush domain the file's writes depend on. The declaration must be
    /// complete; extra domains are safe, a missing one is not (the contract).
    pub fn domains(mut self, domains: impl IntoIterator<Item = FlushDomain>) -> Self {
        self.domains = domains.into_iter().collect();
        self
    }
}

/// A fault, as the layer that observed it reported it (DI-D-50): what failed an operation or a
/// commit, relayed with where it came from.
///
/// Two faults are equal when they name the same source and code and carry the same report -- the
/// one shared `Arc`, as a marking and the completion it records share it -- since an `io::Error`
/// has no equality of its own.
#[derive(Clone, Debug)]
pub struct IoFault {
    /// The layer that reported it: `"windows-ioring"` for dioring over Windows.
    pub source: &'static str,
    /// The layer's code for it, where it gives one. Its meaning is the source's: an `HRESULT` from
    /// the Windows backend, which always gives one.
    pub code: Option<ErrorCode>,
    /// The layer's own report.
    pub error: Arc<io::Error>,
}

impl PartialEq for IoFault {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.code == other.code
            && Arc::ptr_eq(&self.error, &other.error)
    }
}

impl Eq for IoFault {}

/// Why a failure exists, citing where it came from (DI-D-47, DI-D-50). A failed *write* is never a
/// cause (DI-D-12 (a)). The lineages a failure belongs to are those of the writes in its suspect
/// set.
///
/// `Commit` is relayed from the target's backend, as an [`IoFault`] naming its source; the rest
/// are relayed by the consumer or its provider, or observed by dioring, and say so.
#[derive(Clone, Debug)]
pub enum FailureCause<V: Identities> {
    /// The built-in default provider's commit of `target` failed, as the target's backend
    /// reported it.
    Commit {
        /// The target whose commit failed.
        target: IoTargetKey,
        /// The fault, as the backend reported it.
        fault: IoFault,
    },
    /// The consumer imported a failure it learned of outside the instance: relayed by the consumer,
    /// with no code of dioring's.
    Imported {
        /// Where the consumer said it lands.
        scope: ImportScope<V>,
    },
    /// The consumer's provider reported `domain` failed (DI-D-27): relayed by the provider.
    Provider {
        /// The domain the provider failed.
        domain: FlushDomain,
        /// The provider's own error, passed through as it gave it. Any code read from it is the
        /// provider's, not one dioring captured.
        error: Arc<io::Error>,
    },
    /// The consumer's provider dropped `domain`'s completion without answering (DI-D-27): dioring
    /// observed the silence, so there is no error to report.
    ProviderAbandoned {
        /// The domain left unanswered.
        domain: FlushDomain,
    },
}

/// The payload of a `Failed` entry: the one place a token is dispensed besides the inventory.
#[derive(Debug)]
pub struct Failed<V: Identities> {
    /// The failure's identity.
    pub id: V::FailureId,
    /// The token that resolves it.
    pub token: V::FailureToken,
    /// Why it exists.
    pub cause: FailureCause<V>,
    /// The writes it put at risk.
    pub suspect: SuspectSet<V>,
    /// When the instance observed it, on interrupt time, dioring's one time base (DI-D-37). Its
    /// resolution is the system clock tick, so failures observed within one tick carry equal
    /// stamps.
    pub observed: V::Instant,
}

/// What happened to one of a failure's suspect writes after the failure was observed (DI-D-36):
/// an append-only record, so the history can be reconciled after the fact. A marking reports;
/// it never changes the high-water mark, what holds it, or what resolving the failure needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marking<V: Identities> {
    /// The suspect write it is about.
    pub write: V::OpId,
    /// When the instance observed it, on the failure stamps' time base (DI-D-38).
    pub observed: V::Instant,
    /// What happened.
    pub kind: MarkingKind,
}

/// What a [`Marking`] records. Open to further kinds, so match it with a wildcard arm.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MarkingKind {
    /// A nullifier: the write completed with a fault, so it did not happen -- a failed write is an
    /// error on its own completion, and nothing durable covers it (DI-D-12 (a)).
    Nullified {
        /// The fault its completion carried, as the backend reported it (DI-D-47, DI-D-50).
        fault: IoFault,
    },
    /// The write was covered successfully: every part its file depends on answered success
    /// (DI-D-27), so its bytes are durable even though the failure still suspects it. The flags
    /// cite who answered (DI-D-47); a file both answer for has both.
    Covered {
        /// The built-in default flushed the write's file, and dioring observed the flush succeed.
        flushed: bool,
        /// The consumer's durability provider answered success for the file's domains it serves:
        /// the provider's warranty, not something dioring observed.
        provider: bool,
    },
}

/// What a consumer operation was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind<V: Identities> {
    /// A read, which takes no part in durability.
    Read,
    /// A write, tagged with its epoch.
    Write {
        /// The write's epoch.
        epoch: Epoch<V>,
    },
}

/// How a consumer operation ended.
#[derive(Debug)]
pub enum Outcome<V: Identities> {
    /// Completed, with the count its completion reported. A write never completes short: its
    /// target completes it in full or with a fault (DI-D-50); a read may.
    Transferred(u32),
    /// The operation reached its target and failed, as the backend reported it.
    Failed(IoFault),
    /// A gated operation an abandoned epoch stopped before it was released: it never reached the
    /// kernel, so nothing was written. The epoch was its gate, or -- for a write -- its own,
    /// which no write may join once abandoned (DI-D-35, DI-D-43).
    NeverIssued {
        /// The abandoned epoch that stopped it: its gate if that was abandoned, else its own.
        abandoned: Epoch<V>,
    },
}

/// A consumer operation's completion, passed through the instance's queue. `buffer` is `None`
/// for an operation on a registered span: the bytes belong to the registration, not to the
/// operation.
#[derive(Debug)]
pub struct OpCompletion<V: Identities, B, C> {
    /// The operation's identity.
    pub id: V::OpId,
    /// What it was.
    pub kind: OpKind<V>,
    /// How it ended.
    pub outcome: Outcome<V>,
    /// Its owned buffer, or `None` for a registered-span operation.
    pub buffer: Option<B>,
    /// The consumer's context, handed back.
    pub context: C,
}

/// One entry of the instance's completion queue (DI-D-13).
#[derive(Debug)]
pub enum Entry<V: Identities, B, C> {
    /// A consumer operation completed.
    Op(OpCompletion<V, B, C>),
    /// The high-water mark of `through`'s lineage has reached it.
    Durable {
        /// The epoch reached.
        through: Epoch<V>,
    },
    /// A durability failure was observed.
    Failed(Failed<V>),
    /// A request's flushes succeeded, but an unresolved failure at or below `through` prevents
    /// reporting it. Fixed-size and allocation-free, so producing it is cheap.
    Blocked {
        /// The request's epoch.
        through: Epoch<V>,
        /// The failure in the way.
        by: V::FailureId,
    },
    /// The consumer abandoned a failure: its final record.
    Abandoned {
        /// The failure abandoned.
        failure: V::FailureId,
        /// Its suspect set, now declared lost.
        suspect: SuspectSet<V>,
        /// Every marking it gained up to this resolution, in the order observed. A suspect write
        /// still in flight now is reported only by its own completion (DI-D-47).
        markings: Vec<Marking<V>>,
    },
    /// A heal took effect (DI-D-12 (c)): the failure is resolved as healed, and this is its final
    /// record.
    Healed {
        /// The failure healed.
        failure: V::FailureId,
        /// Its suspect set.
        suspect: SuspectSet<V>,
        /// Every marking it gained up to this resolution, in the order observed. A suspect write
        /// still in flight now is reported only by its own completion (DI-D-47).
        markings: Vec<Marking<V>>,
    },
    /// A failure not yet resolved gained a marking.
    Marked {
        /// The failure.
        failure: V::FailureId,
        /// The marking.
        marking: Marking<V>,
    },
    /// `end_lineage` took effect: every epoch of `lineage` not yet durable, through
    /// `abandoned_through`, is abandoned (`None` if none was pending).
    LineageEnded {
        /// The lineage ended.
        lineage: V::Lineage,
        /// The highest id abandoned, if any was.
        abandoned_through: Option<V::EpochId>,
    },
}

/// An epoch's state as the instance sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochState<V: Identities> {
    /// Not sealed; writes may still join it.
    Open,
    /// Sealed, and not yet reported durable or failed.
    Pending,
    /// Sealed, its flushes done, held back by an unresolved failure.
    Blocked(V::FailureId),
    /// Reported durable.
    Durable,
    /// Abandoned by the consumer.
    Abandoned,
}

/// The immediate answer to `make_durable_through`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurabilityRequest<V: Identities> {
    /// A new seal; its answer arrives on the queue as `Durable`, `Failed` or `Blocked`.
    Submitted,
    /// At or below an existing seal: a no-op reporting the epoch's state.
    AlreadySealed(EpochState<V>),
}

/// Why a push was refused before anything was reserved.
#[derive(Debug)]
pub enum PushRefusal<V: Identities> {
    /// The write's epoch is at or below its lineage's seal point (DI-D-9 rule a).
    Sealed {
        /// The write's epoch.
        epoch: Epoch<V>,
        /// Its lineage's seal point.
        sealed_through: V::EpochId,
    },
    /// The write's epoch was abandoned (DI-D-35): its writes were declared lost, so none may join
    /// it, sealed or open. An epoch at or below the seal point is refused as `Sealed` instead.
    EpochAbandoned {
        /// The write's epoch.
        epoch: Epoch<V>,
    },
    /// The write's lineage, or the gate's, is not one the instance can answer for: another
    /// instance's, or one ended or retired (DI-D-40, DI-D-43).
    UnknownLineage(V::Lineage),
    /// The gate names an epoch already abandoned, so the operation could never be released.
    GateAbandoned {
        /// The gate.
        gate: Epoch<V>,
    },
    /// The gate would close a cycle: its lineage reaching it already needs, transitively, this
    /// write to complete (DI-D-23).
    GateCycle {
        /// The cycle, as [`SealError::GateCycle`] lists one, starting at this write's epoch and
        /// its gate.
        cycle: Vec<Epoch<V>>,
    },
    /// No file was given to the instance under this key.
    UnknownFile(IoTargetKey),
    /// A registered-span operation on an instance built without registered buffers.
    NoRegisteredBuffers,
    /// The underlying ring refused the push.
    Ring(io::Error),
}

/// A refused push hands back what it was given: the buffer (`None` for a registered-span
/// operation, which has none) and the consumer's context. `windows-ioring-sys` hands both back
/// on its own refusals (its D-80), so this holds for a `Ring` refusal too.
#[derive(Debug)]
pub struct PushError<V: Identities, B, C> {
    /// Why it was refused.
    pub reason: PushRefusal<V>,
    /// The buffer it took, or `None` for a registered-span operation.
    pub buffer: Option<B>,
    /// The consumer's context.
    pub context: C,
}

/// Whether a write may rest in the system cache (DI-2.6). The contract's own type, so an
/// implementation needs nothing from the ring crate for it; dioring passes it through as the
/// write's flag. It does not change durability: a write-through write is still durable only when
/// its epoch is sealed, because device-level write-through (FUA) is not relied on.
///
/// **What the platform does with it depends on how the file was opened**, and dioring passes the
/// platform's answer through as the write's outcome. Measured on 2026-10-07 through dioring's
/// ring: a write-through write to a handle opened for cached I/O -- including one opened with
/// `FILE_FLAG_WRITE_THROUGH` -- completes as failed, with Win32 error 509 ("not supported on a
/// file opened for cached IO"); to a handle opened with `FILE_FLAG_NO_BUFFERING` as well, it
/// completes as a transfer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WriteCaching {
    /// The write may be satisfied into the system cache.
    #[default]
    Cached,
    /// Ask the system not to leave the data in its cache.
    WriteThrough,
}

/// The advanced parameters of a write. Passed by value and built with setters, like
/// `windows-ioring-sys`' `PushOptions`; the fields are private, so adding one later is not a
/// breaking change. A gate may name an epoch in any lineage of this instance, including another
/// lineage than the write's own.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct WriteOptions<V: Identities> {
    pub(crate) gate: Option<Epoch<V>>,
    pub(crate) caching: WriteCaching,
}

impl<V: Identities> WriteOptions<V> {
    /// No gate, and `WriteCaching::Cached`.
    pub fn new() -> Self {
        Self {
            gate: None,
            caching: WriteCaching::Cached,
        }
    }

    /// Whether the write may rest in the system cache (DI-2.6).
    pub fn caching(mut self, caching: WriteCaching) -> Self {
        self.caching = caching;
        self
    }

    /// Hold the write until `epoch` is durable; it ends as `NeverIssued` if `epoch`, or the
    /// write's own epoch, is abandoned first (contract guarantee 9). `epoch` may be in any lineage
    /// of the instance, the write's own included.
    pub fn gate(mut self, epoch: Epoch<V>) -> Self {
        self.gate = Some(epoch);
        self
    }
}

impl<V: Identities> Default for WriteOptions<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// The advanced parameters of a read. Reads take no part in durability, so a gate is their only
/// option.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct ReadOptions<V: Identities> {
    pub(crate) gate: Option<Epoch<V>>,
}

impl<V: Identities> ReadOptions<V> {
    /// No gate.
    pub fn new() -> Self {
        Self { gate: None }
    }

    /// Hold the read until `epoch` is durable; it ends as `NeverIssued` if `epoch` is abandoned
    /// first. A read takes no part in durability, so it can be on no cycle.
    pub fn gate(mut self, epoch: Epoch<V>) -> Self {
        self.gate = Some(epoch);
        self
    }
}

impl<V: Identities> Default for ReadOptions<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Why `make_durable_through` was refused. Nothing was sealed.
#[derive(Debug, PartialEq, Eq)]
pub enum SealError<V: Identities> {
    /// The lineage is not one the instance can answer for: the handle is another instance's
    /// (DI-D-40).
    UnknownLineage(V::Lineage),
    /// The seal would close a gate cycle (DI-D-43): the writes it must wait for include a held
    /// write whose gate's lineage reaching it needs, transitively, the seal to finish. Seal lower
    /// first, so the gates can be released.
    GateCycle {
        /// The cycle: each held write's epoch followed by the gate it waits for. Each gate's
        /// lineage reaching it needs the next write to complete, and the last gate's needs the
        /// first's.
        cycle: Vec<Epoch<V>>,
    },
}

impl<V: Identities> fmt::Display for SealError<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownLineage(lineage) => {
                write!(
                    f,
                    "{lineage:?} is not a lineage this instance can answer for"
                )
            }
            Self::GateCycle { cycle } => {
                write!(f, "sealing would close a gate cycle: {cycle:?}")
            }
        }
    }
}

impl<V: Identities> std::error::Error for SealError<V> {}

/// How a failure is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The consumer has re-issued what it needs; takes effect, in each lineage, at the first seal
    /// after it that reaches everything pushed there by then (DI-D-46).
    Heal,
    /// The consumer declares the suspect writes lost; takes effect at once.
    Abandon,
}

/// Why a resolution call was refused as a whole.
#[derive(Debug)]
pub enum ResolveRefusal<V: Identities> {
    /// A token another instance minted.
    Foreign(V::FailureId),
}

/// A refused resolution hands every token back.
#[derive(Debug)]
pub struct ResolveError<V: Identities> {
    /// Why it was refused.
    pub reason: ResolveRefusal<V>,
    /// Every token, with the resolution it was given.
    pub returned: Vec<(V::FailureToken, Resolution)>,
}

/// One unresolved failure, as the inventory reports it. Owned, so holding it borrows nothing.
#[derive(Clone, Debug)]
pub struct FailureInfo<V: Identities> {
    /// The failure's identity.
    pub id: V::FailureId,
    /// Why it exists.
    pub cause: FailureCause<V>,
    /// The writes it put at risk.
    pub suspect: SuspectSet<V>,
    /// When the instance observed it: the same stamp its `Failed` entry carried.
    pub observed: V::Instant,
    /// Every marking it has gained, in the order observed.
    pub markings: Vec<Marking<V>>,
    /// Whether its token is held somewhere; if not, the inventory can hand one out.
    pub token_live: bool,
}

/// An operation held for its gate and never issued, handed back by `close()` (DI-2.7).
#[derive(Debug)]
pub struct HeldOperation<V: Identities, B, C> {
    /// The operation's identity.
    pub id: V::OpId,
    /// What it was.
    pub kind: OpKind<V>,
    /// The epoch it was waiting on.
    pub gate: Epoch<V>,
    /// `None` for an operation on a registered span.
    pub buffer: Option<B>,
    /// The consumer's context.
    pub context: C,
}

/// What `close()` hands back once the instance has ended (DI-2.7). Dropping the instance discards
/// the same things.
#[derive(Debug)]
pub struct Leftovers<V: Identities, B, C> {
    /// Gated operations the kernel never saw, in push order.
    pub held: Vec<HeldOperation<V, B, C>>,
    /// Failures still unresolved.
    pub failures: Vec<FailureInfo<V>>,
    /// Every lineage, carrying its final high-water mark.
    pub lineages: Vec<LineageInfo<V>>,
}

/// A refused `add_target`: a file is already present under `key`. The file and its options are
/// handed back.
#[derive(Debug)]
pub struct AddTargetError {
    /// The key already in use.
    pub key: IoTargetKey,
    /// The consumer's file.
    pub file: SharedHandle,
    /// The options it was given with.
    pub options: FileOptions,
}

/// Everything still holding a file that `remove_target` refused, and what clears each hold. Every
/// field is reported, not only the first that applies, so one refusal says all a consumer has
/// to do.
#[derive(Debug)]
pub struct IoTargetBusy<V: Identities> {
    /// The target.
    pub target: IoTargetKey,
    /// Operations the kernel has not completed. Cleared as their completions arrive, which may be
    /// before they are popped (DI-D-45).
    pub in_flight: usize,
    /// Operations held until their gate epoch is durable. Cleared when the gates are released,
    /// or fail as `NeverIssued`.
    pub held_for_gate: usize,
    /// The lowest gate in each lineage gated on.
    pub lowest_gates: Vec<Epoch<V>>,
    /// Writes to the file not yet covered successfully -- named to a provider that answered
    /// success for every domain of the file -- outside any failure's suspect set. A file stays
    /// busy while any lineage has such a write. Cleared by `make_durable_through` at or above
    /// each.
    pub uncovered: usize,
    /// The highest epoch among the uncovered writes, in each lineage that wrote the file.
    pub uncovered_through: Vec<Epoch<V>>,
    /// Unresolved failures whose suspect sets include writes to the file. Cleared by resolving
    /// them: heal and seal again, or abandon.
    pub failures: Vec<V::FailureId>,
}

impl<V: Identities> fmt::Display for IoTargetBusy<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "target {:?} is still in use:", self.target)?;
        if self.in_flight > 0 {
            write!(
                f,
                " {} operation(s) in flight, wait for their completions;",
                self.in_flight
            )?;
        }
        if !self.lowest_gates.is_empty() {
            write!(
                f,
                " {} held until each of {:?} is durable or abandoned;",
                self.held_for_gate, self.lowest_gates
            )?;
        }
        if !self.uncovered_through.is_empty() {
            write!(
                f,
                " {} write(s) not yet covered, seal through each of {:?};",
                self.uncovered, self.uncovered_through
            )?;
        }
        if !self.failures.is_empty() {
            write!(
                f,
                " writes suspect in unresolved failure(s) {:?}, heal or abandon them",
                self.failures
            )?;
        }
        Ok(())
    }
}

/// Why `remove_target` was refused.
#[derive(Debug)]
pub enum RemoveTargetError<V: Identities> {
    /// No file is present under this key.
    UnknownKey(IoTargetKey),
    /// The file is still held; see [`IoTargetBusy`].
    Busy(IoTargetBusy<V>),
}

/// Everything still holding a lineage that `retire_lineage` refused, and what clears each hold.
#[derive(Debug)]
pub struct LineageBusy<V: Identities> {
    /// The lineage.
    pub lineage: V::Lineage,
    /// Writes in the lineage the kernel has not completed. Cleared as their completions arrive,
    /// which may be before they are popped.
    pub in_flight: usize,
    /// Writes in the lineage held for a gate, and operations waiting for one of its epochs. Cleared
    /// when the gates are released, or the operations end as `NeverIssued`.
    pub held_for_gate: usize,
    /// Writes in the lineage not yet covered successfully. Cleared by sealing them.
    pub uncovered: usize,
    /// Unresolved failures in the lineage. Cleared by healing or abandoning them.
    pub failures: Vec<V::FailureId>,
}

/// Why `end_lineage` was refused.
#[derive(Debug)]
pub enum EndLineageRefusal<V: Identities> {
    /// Another instance's lineage.
    Foreign(V::Lineage),
    /// The default lineage cannot be ended; end the instance instead.
    Default,
    /// Another copy of the handle exists; the lineage ends with its last.
    Shared,
}

/// A refused `end_lineage`, handing the handle back.
#[derive(Debug)]
pub struct EndLineageError<V: Identities> {
    /// Why.
    pub reason: EndLineageRefusal<V>,
    /// The handle it was given.
    pub handle: V::LineageHandle,
}

/// Why `retire_lineage` was refused.
#[derive(Debug)]
pub enum RetireLineageRefusal<V: Identities> {
    /// Another instance's lineage.
    Foreign(V::Lineage),
    /// The default lineage always exists.
    Default,
    /// Another copy of the handle exists; only the last can retire the lineage.
    Shared,
    /// The lineage has work outstanding; see [`LineageBusy`].
    Busy(LineageBusy<V>),
}

/// A refused `retire_lineage`, handing the handle back.
#[derive(Debug)]
pub struct RetireLineageError<V: Identities> {
    /// Why.
    pub reason: RetireLineageRefusal<V>,
    /// The handle it was given.
    pub handle: V::LineageHandle,
}
