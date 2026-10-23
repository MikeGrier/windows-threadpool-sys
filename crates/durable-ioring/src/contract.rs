// Copyright (c) 2026 Mike Grier
//! The contract as traits (DI-2.15, DI-D-24 in DESIGN-NOTES.md).
//!
//! Everything in [`crate::types`] that is generic over `V: Identities` is shared by every
//! implementation; the identity types themselves are each implementation's own. The trait grows
//! as the implementation does: it carries only the operations dioring implements, never a
//! method that cannot yet be called.

use core::fmt::Debug;
use core::hash::Hash;
use std::io;

use io_buffers::{IoBuf, IoBufMut, RegisteredSpan};

use crate::io_target::IoTarget;
use crate::key::{ReadKey, TargetKey, WriteKey};
use crate::types::{
    AddTargetError, DurabilityRequest, EndLineageError, Entry, Epoch, EpochState, FailureInfo,
    ImportScope, Imported, IoTargetKey, Leftovers, LineageInfo, PushError, ReadOptions,
    RemoveTargetError, Resolution, ResolveError, RetireLineageError, SealError, Tag,
    UnknownLineage, WriteOptions,
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
    /// When a failure or a marking was observed: a reading of the implementation's clock, which
    /// never decreases (DI-D-37, DI-D-38, DI-D-50).
    type Instant: crate::clock::Instant;

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
    /// The implementation's readiness signal (DI-D-28, DI-D-52 Q6): what a consumer waits on, in
    /// whatever form the platform waits. The Windows driver's is an auto-reset `Event`: an event
    /// rather than a bare handle, so a front end can give it to a thread-pool wait without `unsafe`,
    /// since an `Event` is guaranteed to be an event, never a mutex.
    type Readiness;
    /// The implementation's targets (DI-D-48, DI-D-50): what a write or a read addresses and a
    /// commit makes durable, each stating what it is through [`IoTarget`]. The Windows driver's is
    /// a `Win32FileTarget`.
    type Target: IoTarget;

    /// The readiness signal (DI-D-28), as a value the caller holds: a second holder of the
    /// instance's signal, or a duplicate where the implementation's handles cannot be shared. It
    /// is set after an entry becomes poppable and at least whenever the queue goes from empty to
    /// non-empty, and when a [`pop`](Self::pop) has a failure to report (DI-D-58). One waiter; pop
    /// until `None` after every wake; a wake with nothing to pop is normal.
    ///
    /// # Errors
    ///
    /// The error from making the value, where making it can fail.
    fn readiness(&mut self) -> io::Result<Self::Readiness>;

    /// Give the instance a target after construction, under a key unique within the instance. What
    /// the target states -- its flush domains among it -- is read once, now (DI-2.10 point 3): to
    /// change it, remove the target and add it again.
    ///
    /// # Errors
    ///
    /// An [`AddTargetError`] handing the target back, when one is already present under `key`, or
    /// when `key` is a [`Key<A>`](crate::Key) that declares more access than the target states
    /// (DI-13.10); the error's `cause` says which. An untyped [`IoTargetKey`] declares nothing.
    fn add_target<K: TargetKey>(
        &mut self,
        key: K,
        target: Self::Target,
    ) -> Result<(), AddTargetError<Self::Target>>;

    /// Take a target back, under the key it was given with. Its key may be reused at once. dioring
    /// hands back the `Win32FileTarget` it was given; one given at construction stays open until
    /// the instance ends all the same, because the ring's registration holds its file.
    ///
    /// # Errors
    ///
    /// [`RemoveTargetError::UnknownKey`] when no target is present under `key`, and
    /// [`RemoveTargetError::Busy`] while anything still holds it -- an operation in flight or held
    /// for a gate, a write in any lineage not yet covered successfully, or an unresolved failure
    /// suspecting a write to it -- with every hold reported at once in [`IoTargetBusy`]. Once a
    /// target is gone it cannot be committed, so an epoch with an uncovered write to it could never
    /// become durable.
    ///
    /// [`IoTargetBusy`]: crate::IoTargetBusy
    fn remove_target(
        &mut self,
        key: IoTargetKey,
    ) -> Result<Self::Target, RemoveTargetError<Self::Ids>>;

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
    fn write<K: WriteKey>(
        &mut self,
        file: K,
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
    /// come back on the operation's completion. With a gate in `options`, the write is held until
    /// the gate is durable (guarantee 9), and ends as `NeverIssued` if the gate or its own epoch is
    /// abandoned first.
    ///
    /// # Errors
    ///
    /// A [`PushError`] handing back the buffer and the context, for a file the instance was not
    /// given, a lineage -- the write's or the gate's -- it cannot answer for, a gate already
    /// abandoned, a gate that would close a cycle (DI-D-23), or a ring that refused the push.
    fn write_with<K: WriteKey>(
        &mut self,
        file: K,
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
    fn read<K: ReadKey>(
        &mut self,
        file: K,
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
    /// filled, and the context come back on the operation's completion. With a gate in `options`,
    /// the read is held until the gate is durable, and ends as `NeverIssued` if it is abandoned
    /// first.
    ///
    /// # Errors
    ///
    /// A [`PushError`] handing back the buffer and the context, for a file the instance was not
    /// given, a gate's lineage it cannot answer for, a gate already abandoned, or a ring that
    /// refused the push.
    fn read_with<K: ReadKey>(
        &mut self,
        file: K,
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
    /// An implementation's own failure to make progress, reported where the consumer will see it,
    /// and only when there is no entry to return instead. dioring's are a submission it rang for
    /// again and the kernel refused again, the operations staying queued and the next pop ringing
    /// again; and a ring it could not read the completions of, reported once (DI-D-58). Nothing
    /// retries unprompted, so a cause that stays is the consumer's to answer by dropping the
    /// instance.
    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>>;

    /// Seal every epoch of `through`'s lineage at or below its epoch, and ask for them to be made
    /// durable (DI-D-9). A new seal is answered on the queue (guarantee 4): by `Failed` if one of
    /// its own flushes fails, by `Blocked` if they succeed but an unresolved failure holds an
    /// epoch at or below it, and in every case by `Durable { through }` once the high-water mark
    /// reaches it. A request at or below the seal point is a no-op answered here with the epoch's
    /// state (guarantee 7). A seal waits for the held gated writes it covers, as for writes in
    /// flight.
    ///
    /// # Errors
    ///
    /// A [`SealError`], with nothing sealed: for another instance's lineage, or a seal that would
    /// close a gate cycle (DI-D-43).
    fn make_durable_through(
        &mut self,
        through: Tag<'_, Self::Ids>,
    ) -> Result<DurabilityRequest<Self::Ids>, SealError<Self::Ids>>;

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
    /// failure holds when that lineage's first seal made after this call that reaches the highest
    /// epoch the lineage had been pushed by then finishes successfully (DI-D-41, DI-D-46), or
    /// `Abandon`, effective at once (DI-D-12 (c)). The
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
    /// The answer names the failure, and any domain the scope named that the instance has never
    /// been given (DI-D-46).
    ///
    /// # Errors
    ///
    /// [`UnknownLineage`] for a scope naming a lineage the instance cannot answer for; nothing is
    /// recorded.
    fn import_failure(
        &mut self,
        scope: ImportScope<Self::Ids>,
    ) -> Result<Imported<Self::Ids>, UnknownLineage<Self::Ids>>;

    /// The unresolved failures, in the order they were observed.
    fn failures(&self) -> Vec<FailureInfo<Self::Ids>>;

    /// A token for an unresolved failure whose token is not live -- it was closed or dropped.
    /// `None` otherwise, including for a failure that is resolved, another instance's, or healed
    /// and waiting for its seal.
    fn take_token(&mut self, failure: FailureIdOf<Self>) -> Option<TokenOf<Self>>;

    /// End the instance and hand back what is left (DI-D-25): the gated operations the kernel never
    /// saw, with their buffers and contexts, in push order; the unresolved failures; and every
    /// lineage with its final high-water mark. Seals and flushes nothing. Returns once every
    /// kernel operation and every durability-provider call in flight has completed, as drop does;
    /// entries not yet popped are discarded, and nothing already reported is undone.
    fn close(self) -> Leftovers<Self::Ids, Self::Buffer, Self::Context>
    where
        Self: Sized;
}

/// The registered-buffer extension (DI-D-24): operations on spans of buffers registered with the
/// instance's ring when it was built. An extension because an implementation without a wioring
/// may have none.
pub trait RegisteredBufferRing: DurableRing {
    /// The registered-buffer type.
    type Registered: IoBufMut;

    /// Run `f` on the bytes of registered buffer `i`, and return what it returns. The instance
    /// holds the registration (DI-D-17) and lends the bytes for the call alone, under its own lock:
    /// an implementation whose registration is shared with its delivery can lend them no longer.
    /// While `f` runs, that lock is held, and nothing the lock guards -- completions included --
    /// makes progress.
    ///
    /// # Errors
    ///
    /// Refused while an operation is reading into it, for an index out of range, or on an
    /// instance built without registered buffers; `f` is not called.
    fn with_registered_buffer<T>(&mut self, i: u32, f: impl FnOnce(&[u8]) -> T) -> io::Result<T>;

    /// As [`with_registered_buffer`](Self::with_registered_buffer), with the bytes lent mutably.
    ///
    /// # Errors
    ///
    /// Refused while any operation uses it, for an index out of range, or on an instance built
    /// without registered buffers; `f` is not called.
    fn with_registered_buffer_mut<T>(
        &mut self,
        i: u32,
        f: impl FnOnce(&mut [u8]) -> T,
    ) -> io::Result<T>;

    /// `write_registered_with` with default options.
    ///
    /// # Errors
    ///
    /// As [`write_registered_with`](Self::write_registered_with).
    fn write_registered<K: WriteKey>(
        &mut self,
        file: K,
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
    fn write_registered_with<K: WriteKey>(
        &mut self,
        file: K,
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
    fn read_registered<K: ReadKey>(
        &mut self,
        file: K,
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
    fn read_registered_with<K: ReadKey>(
        &mut self,
        file: K,
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
