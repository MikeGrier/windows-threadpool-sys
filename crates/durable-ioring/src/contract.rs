// Copyright (c) 2026 Mike Grier
//! The contract as traits (DI-2.15, DI-D-24 in DESIGN-NOTES.md).
//!
//! Everything in [`crate::types`] that is generic over `V: Identities` is shared by every
//! implementation; the identity types themselves are each implementation's own. The trait grows
//! as the implementation does: it carries only the operations dioring implements, never a
//! method that cannot yet be called.

use std::fmt::Debug;
use std::hash::Hash;
use std::io;

use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::Event;
use windows_ioring_sys::{IoBuf, IoBufMut, RegisteredSpan};

use crate::types::{
    AddFileError, DurabilityRequest, EndLineageError, Entry, Epoch, EpochState, FailureInfo,
    FileKey, FileOptions, ImportScope, LineageInfo, PushError, ReadOptions, Resolution,
    ResolveError, RetireLineageError, Tag, UnknownLineage, WriteOptions,
};

#[cfg(test)]
mod tests;

/// What dioring requires of an epoch id, the ordered value within a lineage that names an epoch.
/// The consumer chooses the type -- a `u64` (the default), an `Lsn` newtype, a
/// `(segment, offset)` pair -- and dioring only ever compares and stores it. "Id" rather than
/// "number" because it need not be one.
///
/// Two requirements the compiler cannot check, stated in the contract:
/// - `Ord` is a total order that stays consistent for the life of the instance;
/// - values never wrap: a sequence that compares with wraparound (as jbd2's 32-bit transaction
///   ids do) is not a total order, and must be mapped onto one that does not wrap.
pub trait EpochId: Copy + Ord + Debug {}

impl<T: Copy + Ord + Debug> EpochId for T {}

/// The identity types an implementation mints, and the consumer's epoch-id type (DI-D-24). Each
/// identity carries its implementation's private provenance, so only that implementation can
/// make one, and another instance's is refused. A zero-sized marker implements this; the shared
/// types are generic over it, so a consumer names one parameter, `D::Ids`, not five.
pub trait Identities: Copy + Debug + Eq + Hash + 'static {
    /// The consumer's epoch-id type.
    type EpochId: EpochId;
    /// A durability lineage (DI-D-19).
    type Lineage: Copy + Debug + Eq + Hash;
    /// One pushed operation. Ordered by push order within an instance; `None` across instances.
    type OpId: Copy + Debug + Eq + Hash + PartialOrd;
    /// A durability failure's `Copy` identity, used in events and queries.
    type FailureId: Copy + Debug + Eq + Hash;
    /// The only thing that can resolve a failure: affine, move-only, and dropping it is closing
    /// it (DI-D-12).
    type FailureToken: Debug;
    /// A live lineage's handle (DI-D-40): what acts on the lineage. `Clone`; releasing the last
    /// copy ends the lineage.
    type LineageHandle: Clone + Debug;

    /// The failure a token resolves.
    fn token_id(token: &Self::FailureToken) -> Self::FailureId;

    /// The lineage a handle names.
    fn handle_lineage(handle: &Self::LineageHandle) -> Self::Lineage;
}

/// The contract: an instance that takes tagged writes and reports durability by epoch.
///
/// Generic only -- consumers write `D: DurableRing` -- and construction stays on each
/// implementation (DI-D-24).
pub trait DurableRing {
    /// The implementation's identity types, and the consumer's epoch-id type.
    type Ids: Identities;
    /// The owned buffer type of writes and reads.
    type Buffer;
    /// The consumer's per-operation context, handed back with each completion.
    type Context;

    /// A duplicate of the readiness signal (DI-D-28): an auto-reset event the instance owns, set
    /// after an entry becomes poppable and at least whenever the queue goes from empty to
    /// non-empty. One waiter; pop until `None` after every wake; a wake with nothing to pop is
    /// normal.
    ///
    /// An [`Event`] rather than a bare handle, so a front end can give it to a thread-pool wait
    /// without `unsafe`: an `Event` is guaranteed to be an event, never a mutex.
    ///
    /// # Errors
    ///
    /// The error from duplicating the event's handle.
    fn readiness(&mut self) -> io::Result<Event>;

    /// `add_file_with` with default options: no declared flush domains.
    ///
    /// # Errors
    ///
    /// As [`add_file_with`](Self::add_file_with).
    fn add_file(&mut self, key: FileKey, file: SharedHandle) -> Result<(), AddFileError> {
        self.add_file_with(key, file, FileOptions::new())
    }

    /// Give the instance a file after construction, under a key unique within the instance.
    ///
    /// # Errors
    ///
    /// An [`AddFileError`] handing back the file and its options, when a file is already present
    /// under `key`.
    fn add_file_with(
        &mut self,
        key: FileKey,
        file: SharedHandle,
        options: FileOptions,
    ) -> Result<(), AddFileError>;

    /// A handle to the lineage that always exists: a copy of the one construction returned. The
    /// instance holds a copy of its own, so the default lineage never ends while it lives.
    fn default_lineage(&self) -> HandleOf<Self>;

    /// A new lineage, with an optional description for the consumer's own reports (DI-D-19).
    /// Releasing the last copy of its handle ends it (DI-D-40).
    fn mint_lineage(&mut self, description: Option<String>) -> HandleOf<Self>;

    /// Every live lineage, the default among them.
    fn lineages(&self) -> Vec<LineageInfo<Self::Ids>>;

    /// End a lineage through its last handle (DI-D-30): every epoch of it not yet durable is
    /// abandoned, reported by a `LineageEnded` entry, and the lineage is retired once its
    /// operations in flight have completed. Releasing the last handle does the same.
    ///
    /// # Errors
    ///
    /// An [`EndLineageError`] handing the handle back: for another instance's lineage, the default
    /// lineage, or a handle that is not the lineage's last copy.
    fn end_lineage(&mut self, handle: HandleOf<Self>) -> Result<(), EndLineageError<Self::Ids>>;

    /// Retire a lineage through its last handle, abandoning nothing: refused unless nothing of it
    /// is in flight, uncovered, or held by an unresolved failure.
    ///
    /// # Errors
    ///
    /// A [`RetireLineageError`] handing the handle back: for another instance's lineage, the
    /// default lineage, a handle that is not the lineage's last copy, or a lineage with work
    /// outstanding.
    fn retire_lineage(
        &mut self,
        handle: HandleOf<Self>,
    ) -> Result<(), RetireLineageError<Self::Ids>>;

    /// `write_with` with default options: cached.
    ///
    /// # Errors
    ///
    /// As [`write_with`](Self::write_with).
    fn write(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: Self::Buffer,
        tag: Tag<'_, Self::Ids>,
        context: Self::Context,
    ) -> PushResult<Self>
    where
        Self::Buffer: IoBuf,
    {
        self.write_with(file, offset, buffer, tag, context, WriteOptions::new())
    }

    /// Write `buffer` to `file` at `offset`, tagged with `tag`'s epoch. The buffer and the context
    /// come back on the operation's completion.
    ///
    /// # Errors
    ///
    /// A [`PushError`] handing back the buffer and the context, for a file the instance was not
    /// given, another instance's lineage, or a ring that refused the push.
    fn write_with(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: Self::Buffer,
        tag: Tag<'_, Self::Ids>,
        context: Self::Context,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        Self::Buffer: IoBuf;

    /// `read_with` with default options.
    ///
    /// # Errors
    ///
    /// As [`read_with`](Self::read_with).
    fn read(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: Self::Buffer,
        context: Self::Context,
    ) -> PushResult<Self>
    where
        Self::Buffer: IoBufMut,
    {
        self.read_with(file, offset, buffer, context, ReadOptions::new())
    }

    /// Read from `file` at `offset` into `buffer`. Reads take no part in durability. The buffer,
    /// filled, and the context come back on the operation's completion.
    ///
    /// # Errors
    ///
    /// A [`PushError`] handing back the buffer and the context, for a file the instance was not
    /// given or a ring that refused the push.
    fn read_with(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: Self::Buffer,
        context: Self::Context,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        Self::Buffer: IoBufMut;

    /// The next entry, or `None` when the queue is empty. Never waits; wait on
    /// [`readiness`](Self::readiness).
    ///
    /// # Errors
    ///
    /// An implementation's own failure to make progress, reported where the consumer will see it.
    /// dioring's is a submission it retried and the kernel refused again: the operations stay
    /// queued, and the next push or pop retries.
    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>>;

    /// Seal every epoch of `through`'s lineage at or below its epoch, and ask for them to be made
    /// durable (DI-D-9). A new seal is answered on the queue (guarantee 4): by `Failed` if one of
    /// its own flushes fails, by `Blocked` if they succeed but an unresolved failure holds an
    /// epoch at or below it, and in every case by `Durable { through }` once the high-water mark
    /// reaches it. A request at or below the seal point is a no-op answered here with the epoch's
    /// state (guarantee 7).
    ///
    /// # Errors
    ///
    /// For another instance's lineage.
    fn make_durable_through(
        &mut self,
        through: Tag<'_, Self::Ids>,
    ) -> io::Result<DurabilityRequest<Self::Ids>>;

    /// The lineage's high-water mark: the highest epoch id through which every epoch is durable,
    /// or `None` until something is.
    ///
    /// # Errors
    ///
    /// [`UnknownLineage`] for a lineage the instance cannot answer for: another instance's, or one
    /// ended or retired (DI-D-40).
    fn durable_through(
        &self,
        lineage: Lin<Self>,
    ) -> Result<Option<EpochIdOf<Self>>, UnknownLineage<Self::Ids>>;

    /// The lineage's seal point, or `None` until something is sealed.
    ///
    /// # Errors
    ///
    /// As [`durable_through`](Self::durable_through).
    fn sealed_through(
        &self,
        lineage: Lin<Self>,
    ) -> Result<Option<EpochIdOf<Self>>, UnknownLineage<Self::Ids>>;

    /// An epoch's state.
    ///
    /// # Errors
    ///
    /// As [`durable_through`](Self::durable_through), for the epoch's lineage.
    fn epoch_state(
        &self,
        epoch: Epoch<Self::Ids>,
    ) -> Result<EpochState<Self::Ids>, UnknownLineage<Self::Ids>>;

    /// Resolve one or more failures, each by its token: `Heal`, effective in each lineage the
    /// failure holds when that lineage's first seal made after this call finishes successfully
    /// (DI-D-41), or `Abandon`, effective at once (DI-D-12 (c)). The
    /// call is validated whole and applied at one point in observation order.
    ///
    /// # Errors
    ///
    /// For a token another instance minted. Nothing changes, and every token is handed back.
    fn resolve(
        &mut self,
        items: Vec<(TokenOf<Self>, Resolution)>,
    ) -> Result<(), ResolveError<Self::Ids>>;

    /// Record a failure learned of outside the instance (DI-D-12 (h)). Its suspect set is built by
    /// the same rule as any other's, within `scope`, and its token arrives with its `Failed` entry.
    ///
    /// # Errors
    ///
    /// [`UnknownLineage`] for a scope naming a lineage the instance cannot answer for; nothing is
    /// recorded.
    fn import_failure(
        &mut self,
        scope: ImportScope<Self::Ids>,
    ) -> Result<FailureIdOf<Self>, UnknownLineage<Self::Ids>>;

    /// The unresolved failures, in the order they were observed.
    fn failures(&self) -> Vec<FailureInfo<Self::Ids>>;

    /// A token for an unresolved failure whose token is not live -- it was closed or dropped.
    /// `None` otherwise, including for a failure that is resolved, another instance's, or healed
    /// and waiting for its seal.
    fn take_token(&mut self, failure: FailureIdOf<Self>) -> Option<TokenOf<Self>>;
}

/// The registered-buffer extension (DI-D-24): operations on spans of buffers registered with the
/// instance's ring when it was built. An extension because an implementation without a wioring
/// may have none.
pub trait RegisteredBufferRing: DurableRing {
    /// The registered-buffer type.
    type Registered: IoBufMut;

    /// The bytes of registered buffer `i`.
    ///
    /// # Errors
    ///
    /// Refused while an operation is reading into it, for an index out of range, or on an
    /// instance built without registered buffers.
    fn registered_buffer(&mut self, i: u32) -> io::Result<&[u8]>;

    /// The bytes of registered buffer `i`, mutably.
    ///
    /// # Errors
    ///
    /// Refused while any operation uses it, for an index out of range, or on an instance built
    /// without registered buffers.
    fn registered_buffer_mut(&mut self, i: u32) -> io::Result<&mut [u8]>;

    /// `write_registered_with` with default options.
    ///
    /// # Errors
    ///
    /// As [`write_registered_with`](Self::write_registered_with).
    fn write_registered(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        tag: Tag<'_, Self::Ids>,
        context: Self::Context,
    ) -> PushResult<Self> {
        self.write_registered_with(file, offset, span, tag, context, WriteOptions::new())
    }

    /// Write `span` of the registered buffers to `file` at `offset`, tagged with `tag`'s epoch. Its
    /// completion carries no buffer: the bytes belong to the registration.
    ///
    /// # Errors
    ///
    /// A [`PushError`] handing back the context, as for [`DurableRing::write_with`], and for an
    /// instance built without registered buffers or a span the registration does not contain.
    fn write_registered_with(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        tag: Tag<'_, Self::Ids>,
        context: Self::Context,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self>;

    /// `read_registered_with` with default options.
    ///
    /// # Errors
    ///
    /// As [`read_registered_with`](Self::read_registered_with).
    fn read_registered(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        context: Self::Context,
    ) -> PushResult<Self> {
        self.read_registered_with(file, offset, span, context, ReadOptions::new())
    }

    /// Read from `file` at `offset` into `span` of the registered buffers.
    ///
    /// # Errors
    ///
    /// As [`write_registered_with`](Self::write_registered_with), less the epoch.
    fn read_registered_with(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        context: Self::Context,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self>;
}

/// An implementation's lineage type.
pub type Lin<D> = <<D as DurableRing>::Ids as Identities>::Lineage;
/// An implementation's operation identity.
pub type OpIdOf<D> = <<D as DurableRing>::Ids as Identities>::OpId;
/// An implementation's failure identity.
pub type FailureIdOf<D> = <<D as DurableRing>::Ids as Identities>::FailureId;
/// An implementation's failure token.
pub type TokenOf<D> = <<D as DurableRing>::Ids as Identities>::FailureToken;
/// An implementation's lineage handle.
pub type HandleOf<D> = <<D as DurableRing>::Ids as Identities>::LineageHandle;
/// An implementation's epoch-id type.
pub type EpochIdOf<D> = <<D as DurableRing>::Ids as Identities>::EpochId;
/// An implementation's queue entry.
pub type EntryOf<D> =
    Entry<<D as DurableRing>::Ids, <D as DurableRing>::Buffer, <D as DurableRing>::Context>;
/// What an implementation's push returns.
pub type PushResult<D> = Result<
    OpIdOf<D>,
    PushError<<D as DurableRing>::Ids, <D as DurableRing>::Buffer, <D as DurableRing>::Context>,
>;
