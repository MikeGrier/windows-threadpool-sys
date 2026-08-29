// Copyright (c) 2026 Mike Grier
//! `Batch`: a scoped, exclusive submission window (M3.1-M3.3, M3.5, M5).

use std::ffi::c_void;
use std::io;
use std::mem::ManuallyDrop;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingCancelRequest, BuildIoRingFlushFile, BuildIoRingReadFile,
    BuildIoRingRegisterBuffers, BuildIoRingRegisterFileHandles, BuildIoRingWriteFile,
    FILE_FLUSH_DATA, FILE_FLUSH_DEFAULT, FILE_FLUSH_MIN_METADATA, FILE_FLUSH_MODE,
    FILE_FLUSH_NO_SYNC, FILE_WRITE_FLAGS, FILE_WRITE_FLAGS_NONE, FILE_WRITE_FLAGS_WRITE_THROUGH,
    IORING_BUFFER_INFO, IORING_BUFFER_REF, IORING_BUFFER_REF_0, IORING_HANDLE_REF,
    IORING_HANDLE_REF_0, IORING_REF_RAW, IORING_REF_REGISTERED, IORING_REGISTERED_BUFFER,
    IORING_SQE_FLAGS, IOSQE_FLAGS_DRAIN_PRECEDING_OPS, IOSQE_FLAGS_NONE, SubmitIoRing,
};

use crate::buf::{IoBuf, IoBufMut};
use crate::error::check;
use crate::ring::{Completion, IoRing, Op, RingId};
use crate::token::Token;

/// Per-push options shared across every op builder (M3.2).
#[derive(Clone, Copy, Debug, Default)]
#[must_use]
pub struct PushOptions {
    drain_preceding: bool,
}

impl PushOptions {
    /// The default options: no barrier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `IOSQE_FLAGS_DRAIN_PRECEDING_OPS`: this op does not start until
    /// every op already queued on this batch's ring has completed. A
    /// barrier, not a cheap tag -- it forces the ring to drain before
    /// continuing.
    ///
    /// # Ring-wide, and it spans submissions
    ///
    /// Measured, not inferred (D-24 in `DESIGN-NOTES.md`). The barrier
    /// reaches every operation outstanding on the *ring*, not only the ones
    /// queued in this batch, and it holds back ops pushed after it even when
    /// they target an entirely different file -- which rules out
    /// filesystem-level serialization as the explanation. Results were
    /// identical whether the sequence went in one [`Batch::submit`] or three.
    ///
    /// The consequence to plan for: **cross-epoch pipelining through a single
    /// ring is not available.** A consumer that closes an epoch with a
    /// drained flush stalls that whole ring for the flush's duration, so the
    /// way to overlap epochs is more rings, not more batches.
    ///
    /// # The barrier stops at the ring's edge
    ///
    /// It orders SQEs against SQEs, and nothing else. Any non-ring I/O -- an
    /// overlapped `DeviceIoControl`, anything issued through
    /// `windows-overlapped-io-sys`, a plain blocking write -- is outside it in
    /// *both* directions: this flag can neither make a ring op wait for a
    /// non-ring op nor make a non-ring op wait for ring ops.
    ///
    /// A consumer mixing both paths is the normal case rather than an exotic
    /// one, and it must enforce that ordering in its own code. What this
    /// crate offers toward it is [`IoRing::completion_event`], which makes
    /// waiting on the ring *alongside* other handles expressible without
    /// giving up the ring or blocking a thread in a drain.
    ///
    /// # Ordering, never durability by itself
    ///
    /// This flag orders operations; it does not flush anything. It is,
    /// however, what makes a flush cover the writes before it: a flush pushed
    /// after a batch of writes *without* this flag routinely completes while
    /// many of those writes are still outstanding (D-23 in `DESIGN-NOTES.md`,
    /// measured at 17 and 23 of 32 writes finishing after the flush did). So
    /// the obvious spelling -- push the writes, then push a flush -- silently
    /// does not make those writes durable.
    ///
    /// A flush does not take its barrier decision from here, for exactly that
    /// reason: [`Batch::flush`] requires a [`FlushCoverage`] instead, so the
    /// choice cannot be inherited from a default (M12.1, D-25).
    pub fn drain_preceding(mut self, drain: bool) -> Self {
        self.drain_preceding = drain;
        self
    }

    fn sqe_flags(self) -> IORING_SQE_FLAGS {
        if self.drain_preceding {
            IOSQE_FLAGS_DRAIN_PRECEDING_OPS
        } else {
            IOSQE_FLAGS_NONE
        }
    }
}

/// Whether a flush covers the operations queued before it (M12.1, D-23 in
/// `DESIGN-NOTES.md`).
///
/// This is a **required argument** on [`Batch::flush`] and
/// [`Batch::flush_raw`] rather than a field of [`PushOptions`], because there
/// is no defensible default. An unflagged flush does *not* cover preceding
/// writes: it is an ordinary operation competing with them, and it frequently
/// wins. A default would make the obvious spelling -- push the writes, then
/// push a flush -- a silent data-loss bug rather than a missing feature, so
/// the decision is taken at the call site instead (D-25).
///
/// It is an enum rather than a `bool` for the same reason: `flush(&file,
/// true)` does not say what the `true` decides, and this is not a parameter
/// anyone should have to look up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushCoverage {
    /// Wait for every operation already outstanding on the ring, then flush.
    ///
    /// This is what makes preceding writes durable when the flush's
    /// completion is observed, and it is what a caller closing an epoch
    /// wants.
    ///
    /// It sets `IOSQE_FLAGS_DRAIN_PRECEDING_OPS`, which is a **ring-wide**
    /// barrier rather than a per-file one (D-24, measured): operations pushed
    /// after it are held until it completes even when they target unrelated
    /// files, and its reach is every operation outstanding on the ring, not
    /// only the ones in this batch. The whole ring stalls for the flush's
    /// duration. That cost is real, and it is the only reason the other
    /// variant exists.
    CoversPrecedingOperations,
    /// Queue the flush with no barrier: it may start, and complete, while
    /// writes pushed before it are still in flight.
    ///
    /// **Almost never what a caller wants.** Its completion proves nothing
    /// about any preceding write, so using it to close an epoch loses data
    /// that the caller believes is committed -- invisibly, until power is
    /// lost.
    ///
    /// Two uses are legitimate. One is *host sequencing*: the caller has
    /// already observed the completions of every write in the epoch before
    /// pushing this, so the ordering is established outside the ring and the
    /// barrier would only add a stall. The other is a flush that is not being
    /// used for durability at all.
    Unordered,
}

impl FlushCoverage {
    fn sqe_flags(self) -> IORING_SQE_FLAGS {
        match self {
            Self::CoversPrecedingOperations => IOSQE_FLAGS_DRAIN_PRECEDING_OPS,
            Self::Unordered => IOSQE_FLAGS_NONE,
        }
    }
}

/// Where a write is allowed to stop: in the system cache, or past it (M12.3,
/// D-25 in `DESIGN-NOTES.md`).
///
/// This is `BuildIoRingWriteFile`'s `FILE_WRITE_FLAGS` parameter, which the
/// crate previously hardcoded to `FILE_WRITE_FLAGS_NONE` -- so a consumer
/// reading this API saw ordering but no way to express anything about caching
/// at all, and could reasonably conclude the ring did not expose it. It does;
/// the crate was narrowing the platform to what its own examples needed.
///
/// # What write-through is, and what it is not
///
/// Worth stating precisely, because conflating these produced a wrong
/// recommendation in the exchange that prompted this work.
///
/// It **is** a first-level cache directive: it tells the OS not to leave the
/// data sitting in the system cache. Its practical value is latency shaping --
/// data already at the device makes a later flush shorter, which can matter
/// when the flush is on a commit path.
///
/// It is **not** a durability guarantee, and it is **not** FUA. Whether it
/// becomes a Force Unit Access bit on the underlying command depends on the
/// driver, the volume, and whether the device's write cache is enabled -- none
/// of which this API can see or promise. A write that completes with
/// [`WriteCaching::WriteThrough`] may still be sitting in a volatile device
/// cache, and will be lost on power failure like any other.
///
/// **The flush operation is the only durability primitive the ring has**, and
/// only with [`FlushCoverage::CoversPrecedingOperations`]. Reaching for
/// write-through instead of a covering flush is the mistake this doc exists to
/// prevent; reaching for it *as well* is a legitimate latency optimization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WriteCaching {
    /// `FILE_WRITE_FLAGS_NONE`: the write may be satisfied into the system
    /// cache, and reaching the device is a later flush's job.
    ///
    /// The default, and the right choice for the streaming writes of an epoch
    /// that a covering flush will close.
    #[default]
    Cached,
    /// `FILE_WRITE_FLAGS_WRITE_THROUGH`: ask the OS not to leave the data in
    /// the system cache.
    ///
    /// A latency-shaping knob, not a durability marker -- see this type's
    /// docs. It does not remove the need for a flush.
    WriteThrough,
}

impl WriteCaching {
    fn raw(self) -> FILE_WRITE_FLAGS {
        match self {
            Self::Cached => FILE_WRITE_FLAGS_NONE,
            Self::WriteThrough => FILE_WRITE_FLAGS_WRITE_THROUGH,
        }
    }
}

/// How much a flush is asked to push, and whether it syncs the device at all
/// (M12.4, D-25 in `DESIGN-NOTES.md`).
///
/// This is `BuildIoRingFlushFile`'s `FILE_FLUSH_MODE` parameter, which the
/// crate previously hardcoded to `FILE_FLUSH_DEFAULT`. Exposing it is the
/// other half of D-25: the kernel offers a durability parameter here and a
/// wrapper that hides it narrows the platform to what its own examples needed.
///
/// [`FlushMode::Default`] is what a caller closing an epoch wants, and is
/// what the crate did before this existed.
///
/// # `NoSync` is not a durability operation
///
/// Three of these four modes issue the device sync; [`FlushMode::NoSync`] does
/// not. That is worth stating rather than inferring, and the inference runs
/// the other way too: **the existence of a distinct "no sync" mode is the
/// evidence that the other three do sync.** Nothing in the Win32
/// documentation says so directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlushMode {
    /// `FILE_FLUSH_DEFAULT`: flush the file's data and metadata, and sync the
    /// device.
    ///
    /// The default, and the mode a durability barrier wants. Pair it with
    /// [`FlushCoverage::CoversPrecedingOperations`] or it covers nothing.
    #[default]
    Default,
    /// `FILE_FLUSH_DATA`: flush the file's data, and sync the device.
    ///
    /// Metadata that is not required to read the data back may be left
    /// unflushed. The analogue of `fdatasync`, and the usual choice for a log
    /// whose records are self-describing.
    Data,
    /// `FILE_FLUSH_MIN_METADATA`: flush the data plus the minimum metadata
    /// needed to retrieve it, and sync the device.
    MinMetadata,
    /// `FILE_FLUSH_NO_SYNC`: flush to the device **without** issuing the sync.
    ///
    /// **This is the one mode that makes nothing durable.** It pushes data out
    /// of the system cache and stops there, so anything still held in a
    /// volatile device cache is lost on power failure exactly as if no flush
    /// had been issued at all. A completion from this mode is not a commit
    /// point and must never be reported to a caller as one.
    ///
    /// It is useful for shaping when cache pressure is paid -- moving data
    /// deviceward early so a later real flush is short -- which is the same
    /// role [`WriteCaching::WriteThrough`] plays on the write side, and it
    /// carries the same warning: a latency knob, not a durability primitive.
    NoSync,
}

impl FlushMode {
    fn raw(self) -> FILE_FLUSH_MODE {
        match self {
            Self::Default => FILE_FLUSH_DEFAULT,
            Self::Data => FILE_FLUSH_DATA,
            Self::MinMetadata => FILE_FLUSH_MIN_METADATA,
            Self::NoSync => FILE_FLUSH_NO_SYNC,
        }
    }
}

/// Build the raw handle reference for `file`, refusing a [`RegisteredFile`]
/// that did not come from `ring_id`'s own ring (PR #20 review response): the
/// index alone names a slot in *some* ring's file table, and a slot from a
/// different ring can legitimately hold an entirely different file, so
/// crossing rings must be an explicit, reported error rather than silently
/// addressing the wrong handle.
fn handle_ref(file: FileRef, ring_id: RingId) -> io::Result<IORING_HANDLE_REF> {
    match file {
        FileRef::Raw(handle) => Ok(IORING_HANDLE_REF {
            Kind: IORING_REF_RAW,
            Handle: IORING_HANDLE_REF_0 { Handle: handle },
        }),
        FileRef::Registered(file) => {
            if file.ring_id != ring_id {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "this RegisteredFile was registered on a different IoRing",
                ));
            }
            Ok(IORING_HANDLE_REF {
                Kind: IORING_REF_REGISTERED,
                Handle: IORING_HANDLE_REF_0 {
                    Index: file.index(),
                },
            })
        }
    }
}

fn raw_buffer_ref(address: *mut c_void) -> IORING_BUFFER_REF {
    IORING_BUFFER_REF {
        Kind: IORING_REF_RAW,
        Buffer: IORING_BUFFER_REF_0 { Address: address },
    }
}

fn registered_buffer_ref(index: u32, offset: u32) -> IORING_BUFFER_REF {
    IORING_BUFFER_REF {
        Kind: IORING_REF_REGISTERED,
        Buffer: IORING_BUFFER_REF_0 {
            IndexAndOffset: IORING_REGISTERED_BUFFER {
                BufferIndex: index,
                Offset: offset,
            },
        },
    }
}

fn checked_len(len: usize) -> io::Result<u32> {
    u32::try_from(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("an IoRing buffer is limited to u32::MAX bytes; {len} does not fit"),
        )
    })
}

/// A file, addressed either by a raw `HANDLE` or by the index a prior
/// [`Batch::register_files`] assigned it (M5.1).
///
/// This enum is the `_raw` pushes' addressing parameter. A safe push takes a
/// [`FileTarget`] instead -- a [`SharedFile`] or a [`RegisteredFile`] -- so
/// reaching a registered file has not required `unsafe` since M10.4 (D-29,
/// D-33). Constructing a `FileRef` directly is only necessary for the `_raw`
/// pushes, whose reason to exist is a bare `HANDLE` this crate cannot keep
/// alive for you.
///
/// A distinct type per addressing mode -- rather than one method accepting
/// either a `HANDLE` or a bare `u32` -- is what makes "read against a
/// registered file" and "read against an unregistered one" impossible to
/// confuse by accident: a [`RegisteredFile`] cannot be mistaken for a raw
/// `HANDLE`, or vice versa, because they are different types.
#[derive(Clone, Copy, Debug)]
pub enum FileRef {
    /// `IORING_REF_RAW`: the caller's own open handle.
    Raw(HANDLE),
    /// `IORING_REF_REGISTERED`: an index from [`Batch::register_files`].
    Registered(RegisteredFile),
}

impl From<HANDLE> for FileRef {
    fn from(handle: HANDLE) -> Self {
        FileRef::Raw(handle)
    }
}

impl From<RegisteredFile> for FileRef {
    fn from(file: RegisteredFile) -> Self {
        FileRef::Registered(file)
    }
}

/// A file handle this crate can read/write through without the caller
/// having to prove it outlives every operation pushed against it (M8, PR
/// #20 review response).
///
/// Backed by `Arc<OwnedHandle>` rather than [`Token`]'s exclusive-ownership
/// shape: unlike a buffer, one handle is legitimately the target of many
/// concurrent pushes, so what must survive until every one of them
/// completes is a *reference*, not sole ownership. Every safe push method
/// (e.g. [`Batch::read`], as opposed to its `_raw` sibling) clones this
/// `Arc` into the same [`Token`] that already tracks the operation's own
/// payload, so the underlying handle survives until that token is claimed
/// or leaked (D-4 in `DESIGN-NOTES.md`), regardless of what the caller does
/// with its own clone.
#[derive(Clone, Debug)]
pub struct SharedFile(Arc<OwnedHandle>);

impl SharedFile {
    /// Wrap an owned handle so it can be read/written through the ring
    /// safely.
    #[must_use]
    pub fn new(handle: OwnedHandle) -> Self {
        Self(Arc::new(handle))
    }

    fn raw_handle(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}

impl From<OwnedHandle> for SharedFile {
    fn from(handle: OwnedHandle) -> Self {
        Self::new(handle)
    }
}

/// One index a [`Batch::register_files`] registration assigned (M5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredFile {
    index: u32,
    ring_id: RingId,
}

impl RegisteredFile {
    /// The raw registered-file index.
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::SharedFile {}
    impl Sealed for super::RegisteredFile {}
}

/// A file a *safe* push can address: either a [`SharedFile`] or a
/// [`RegisteredFile`] (M10.4; D-29 and D-33 in `DESIGN-NOTES.md`).
///
/// This exists because the two carry different lifetime obligations, and
/// exactly one of them has none. A `SharedFile` is a raw `HANDLE` underneath,
/// so something must keep it open until the kernel is finished; a
/// `RegisteredFile` is an index into a table the ring itself owns, minted by
/// this crate and checked against the minting ring, so there is nothing for a
/// caller to keep alive. Both are therefore safe to push, and neither needs
/// `unsafe` -- which is why [`Batch::read`] and its siblings are generic over
/// this trait rather than hardcoding `SharedFile`.
///
/// [`Guard`](FileTarget::Guard) is what the returned [`Token`] holds until the
/// operation's completion is observed. For `SharedFile` that is a clone of its
/// `Arc`, which is what makes the handle outlive the operation; for
/// `RegisteredFile` there is nothing to keep alive, so it is the (`Copy`)
/// index itself, handed back for symmetry rather than out of necessity.
///
/// **Sealed**: this crate implements it for exactly those two types and no
/// others. An outside implementation could return an arbitrary raw `HANDLE`
/// from [`as_file_ref`](FileTarget::as_file_ref) with no guard keeping it
/// alive, which is precisely the unsoundness the `_raw` pushes make a caller
/// take responsibility for with `unsafe`.
pub trait FileTarget: sealed::Sealed {
    /// What the operation's [`Token`] must hold until its completion is
    /// observed.
    type Guard: Send + 'static;

    /// How this target addresses its file.
    fn as_file_ref(&self) -> FileRef;

    /// Produce the value the [`Token`] will hold for the operation's
    /// duration.
    fn guard(&self) -> Self::Guard;
}

impl FileTarget for SharedFile {
    type Guard = SharedFile;

    fn as_file_ref(&self) -> FileRef {
        FileRef::Raw(self.raw_handle())
    }

    fn guard(&self) -> Self::Guard {
        self.clone()
    }
}

impl FileTarget for RegisteredFile {
    type Guard = RegisteredFile;

    fn as_file_ref(&self) -> FileRef {
        FileRef::Registered(*self)
    }

    fn guard(&self) -> Self::Guard {
        *self
    }
}

/// The confirmed result of a [`Batch::register_files`] push: `count`
/// contiguous indices starting at `base_index`.
///
/// `base_index` is currently always zero, because a ring accepts at most one
/// registration that assigns an index -- do not read its presence as evidence
/// that several registrations compose. It is kept rather than folded away
/// because it is the correct shape if that rule is ever relaxed (M10.3, D-31).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredFiles {
    base_index: u32,
    count: u32,
    ring_id: RingId,
}

impl RegisteredFiles {
    /// How many handles this registration covers.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.count
    }

    /// Whether this registration covers no handles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The [`RegisteredFile`] for the `i`-th handle passed to
    /// [`Batch::register_files`], or `None` if `i` is out of range.
    #[must_use]
    pub fn get(&self, i: u32) -> Option<RegisteredFile> {
        (i < self.count).then(|| RegisteredFile {
            index: self.base_index + i,
            ring_id: self.ring_id,
        })
    }
}

/// A [`Batch::register_files`] push not yet matched to its completion.
#[derive(Debug)]
pub struct PendingFileRegistration {
    user_data: usize,
    base_index: u32,
    count: u32,
    ring_id: RingId,
}

impl PendingFileRegistration {
    /// This push's `UserData` identity, to match against a popped
    /// [`Completion`].
    #[must_use]
    pub fn user_data(&self) -> usize {
        self.user_data
    }

    /// Turn this pending registration into usable [`RegisteredFiles`] once
    /// `completion` names it, or hand it back unchanged if `completion`
    /// names a different operation.
    ///
    /// # Errors
    ///
    /// The inner `Result` is `Err` if the registration itself failed --
    /// nothing is lost, since no owned resource was ever handed over for
    /// this op (M5.1, unlike [`Token`]).
    pub fn claim_if(self, completion: &Completion) -> Result<io::Result<RegisteredFiles>, Self> {
        if completion.user_data() != self.user_data || completion.ring_id() != self.ring_id {
            return Err(self);
        }
        Ok(completion.result().map(|_| RegisteredFiles {
            base_index: self.base_index,
            count: self.count,
            ring_id: self.ring_id,
        }))
    }
}

/// A marker a [`Token`] owns while a registered-buffer-indexed read or write
/// is outstanding (M5.2, M5.3).
///
/// Decrements [`RegisteredBuffers`]'s own count only when actually claimed:
/// dropping an unclaimed token forgets this like any other value a `Token`
/// holds, which correctly leaves the registration believing the use is
/// still outstanding, since nothing proved otherwise.
pub struct RegisteredUse(Arc<AtomicUsize>);

impl Drop for RegisteredUse {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Buffers registered with a ring so later reads and writes can address them
/// by index instead of handing over a fresh owned buffer each call (M5.2).
///
/// Win32's `IoRing` has no unregister call at all: once registered, an
/// index is valid for the ring's remaining life. This type's own job is
/// narrower than reproducing that -- refusing to let its *backing memory*
/// be freed while this crate still has an operation outstanding against it
/// (M5.3) -- not proving an index will never be touched again by some
/// later, differently-built SQE. That wider hazard belongs to
/// [`crate::IoRing::push_raw`]'s existing `unsafe` contract, the same as any
/// raw use of an index this crate handed out.
pub struct RegisteredBuffers<B: IoBufMut> {
    buffers: ManuallyDrop<Vec<B>>,
    base_index: u32,
    outstanding: Arc<AtomicUsize>,
    ring_id: RingId,
}

impl<B: IoBufMut> RegisteredBuffers<B> {
    /// How many buffers this registration holds.
    #[must_use]
    pub fn len(&self) -> u32 {
        u32::try_from(self.buffers.len()).unwrap_or(u32::MAX)
    }

    /// Whether this registration holds no buffers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    /// Borrow the `i`-th buffer directly, for reading a completed read's or
    /// write's bytes without going through the ring at all.
    #[must_use]
    pub fn get(&self, i: u32) -> Option<&B> {
        self.buffers.get(i as usize)
    }

    /// The registered index of the `i`-th buffer, or an
    /// [`io::ErrorKind::InvalidInput`] error if out of range.
    fn checked_index(&self, i: u32) -> io::Result<u32> {
        if i < self.len() {
            Ok(self.base_index + i)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("buffer index {i} is out of range for this registration"),
            ))
        }
    }

    /// As [`RegisteredBuffers::checked_index`], plus checking that
    /// `span.offset .. span.offset + span.len` actually fits inside that
    /// buffer -- validating only the index and handing an unchecked range to
    /// the kernel would let it read or write outside the registered
    /// allocation.
    fn checked_span(&self, span: RegisteredSpan) -> io::Result<u32> {
        let index = self.checked_index(span.buffer_index)?;
        let buffer = self
            .buffers
            .get(span.buffer_index as usize)
            .expect("checked_index just validated this index");
        let bytes_len = buffer.bytes_len();
        let fits = u64::from(span.offset)
            .checked_add(u64::from(span.len))
            .is_some_and(|end| end <= bytes_len as u64);
        if !fits {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "span offset {} + len {} does not fit inside buffer {} ({bytes_len} bytes)",
                    span.offset, span.len, span.buffer_index
                ),
            ));
        }
        Ok(index)
    }
}

impl<B: IoBufMut> Drop for RegisteredBuffers<B> {
    fn drop(&mut self) {
        if self.outstanding.load(Ordering::SeqCst) > 0 {
            // Refused, not silently permitted (M5.3): freeing now would
            // leave an outstanding `IORING_BUFFER_REF` pointing at freed
            // memory. Loud in debug builds; in release, leaking is the safe
            // failure mode -- the same choice `Token` already makes
            // ("leak is safe, use-after-free is not"), so `buffers` is
            // simply never reclaimed rather than freed out from under a
            // still-outstanding op.
            debug_assert!(
                false,
                "RegisteredBuffers dropped while an operation still references it"
            );
            return;
        }
        // SAFETY: `outstanding == 0`, so no operation still references these
        // buffers, and this is the only place `buffers` is ever dropped.
        unsafe { ManuallyDrop::drop(&mut self.buffers) };
    }
}

/// Which bytes of one [`RegisteredBuffers`] entry an op reads or writes
/// (M5.2): bundled into one value so `read_registered`/`write_registered`
/// stay under a sane argument count.
#[derive(Clone, Copy, Debug)]
pub struct RegisteredSpan {
    /// Which buffer within the registration, per [`RegisteredBuffers::get`].
    pub buffer_index: u32,
    /// The starting byte offset within that buffer.
    pub offset: u32,
    /// How many bytes to read or write.
    pub len: u32,
}

/// A [`Batch::register_buffers`] push not yet matched to its completion.
///
/// `buffers` is `ManuallyDrop` and this type's own `Drop` is deliberately
/// empty, mirroring [`Token`] (PR #20 review response): the registration is
/// already queued via `BuildIoRingRegisterBuffers` the instant
/// [`Batch::register_buffers`] returns, before any completion is observed,
/// so a caller that drops this without ever matching a completion has no
/// proof the kernel is done deciding whether to retain these addresses.
/// Freeing them here anyway would risk handing memory the kernel still
/// references back to the allocator; leaking is the safe failure mode, the
/// same choice `Token` and [`RegisteredBuffers`] both already make.
pub struct PendingBufferRegistration<B: IoBufMut> {
    user_data: usize,
    base_index: u32,
    buffers: ManuallyDrop<Vec<B>>,
    ring_id: RingId,
}

impl<B: IoBufMut> PendingBufferRegistration<B> {
    /// This push's `UserData` identity, to match against a popped
    /// [`Completion`].
    #[must_use]
    pub fn user_data(&self) -> usize {
        self.user_data
    }

    /// Turn this pending registration into usable [`RegisteredBuffers`] once
    /// `completion` names it, or hand it back unchanged if `completion`
    /// names a different operation.
    ///
    /// # Errors
    ///
    /// The inner `Result` is `Err` if the registration itself failed; the
    /// buffers are dropped normally in that case, exactly as if they had
    /// never been registered -- a matched completion, success or failure, is
    /// exactly the proof this type's `Drop` is waiting for.
    pub fn claim_if(
        mut self,
        completion: &Completion,
    ) -> Result<io::Result<RegisteredBuffers<B>>, Self> {
        if completion.user_data() != self.user_data || completion.ring_id() != self.ring_id {
            return Err(self);
        }
        // SAFETY: `completion` names this exact registration, so the kernel
        // has already decided whether to retain these addresses -- the
        // condition this type's `Drop` would otherwise wait forever for.
        let buffers = unsafe { ManuallyDrop::take(&mut self.buffers) };
        let base_index = self.base_index;
        let ring_id = self.ring_id;
        Ok(completion.result().map(move |_| RegisteredBuffers {
            buffers: ManuallyDrop::new(buffers),
            base_index,
            outstanding: Arc::new(AtomicUsize::new(0)),
            ring_id,
        }))
    }
}

impl<B: IoBufMut> Drop for PendingBufferRegistration<B> {
    fn drop(&mut self) {
        // Deliberately empty; see this type's own doc comment.
    }
}

impl<B: IoBufMut> std::fmt::Debug for PendingBufferRegistration<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not derived: deriving would require `B: Debug`, which a caller's
        // buffer type need not satisfy.
        f.debug_struct("PendingBufferRegistration")
            .field("user_data", &self.user_data)
            .field("base_index", &self.base_index)
            .finish_non_exhaustive()
    }
}

/// A scoped, exclusive submission window over one [`IoRing`] (D-5).
///
/// Holding `&mut IoRing` for its lifetime is what turns Win32's own
/// "you must serialize submission" footnote into a compiler-enforced
/// guarantee: two batches over the same ring cannot coexist. Every push
/// queues its SQE the instant it succeeds -- there is no rewind -- so
/// `Batch` submits on [`Drop`] rather than leaving queued SQEs for some
/// later, unrelated submit to discover (D-5).
pub struct Batch<'ring> {
    ring: &'ring mut IoRing,
    submitted: bool,
}

impl<'ring> Batch<'ring> {
    /// Open a batch over `ring`.
    pub fn new(ring: &'ring mut IoRing) -> Self {
        Self {
            ring,
            submitted: false,
        }
    }

    fn require(&self, op: Op) -> io::Result<()> {
        if self.ring.supports(op) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("this ring does not support {op:?}"),
            ))
        }
    }

    /// Queue a read of `buffer.bytes_len()` bytes from `file` at `offset`.
    ///
    /// Prefer [`Batch::read`] unless `file` needs to address a raw
    /// `FileRef` directly, without `SharedFile`'s `Arc` bookkeeping.
    ///
    /// # Safety
    ///
    /// If `file` is [`FileRef::Raw`], the handle must be valid, opened with
    /// read access, and must remain valid -- not closed, not reused for a
    /// different object -- until this operation's completion is observed
    /// (via a popped [`Completion`] or [`Token::claim_if`]) or until the
    /// ring runs down (M8, PR #20 review response). A [`FileRef::Registered`]
    /// target needs none of this.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the ring was not probed as
    /// supporting [`Op::Read`]; [`io::ErrorKind::InvalidInput`] if the
    /// buffer is longer than `u32::MAX`; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room (M3.3, not
    /// auto-flushed -- see [`Batch`]'s own docs); or any other error from
    /// `BuildIoRingReadFile`. On any error the buffer is dropped normally,
    /// not leaked or handed back.
    pub unsafe fn read_raw<B: IoBufMut>(
        &mut self,
        file: impl Into<FileRef>,
        mut buffer: B,
        offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<B>> {
        self.require(Op::Read)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_mut_ptr().cast::<c_void>();
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let token = Token::new(self.ring, buffer)?;
        let user_data = token.id();
        // SAFETY: `self.ring`'s handle is live; `address` is `IoBufMut`'s
        // promised stable, exclusively-owned pointer, valid for `len` bytes
        // until `token` is claimed; `file` is the caller's to keep alive,
        // forwarded from this function's own contract.
        let hr = unsafe {
            BuildIoRingReadFile(
                self.ring.raw_handle(),
                target,
                raw_buffer_ref(address),
                len,
                offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// As [`Batch::read_raw`], but safe: `file` is a [`SharedFile`] rather
    /// than a bare [`FileRef`], so the pushed operation keeps its own clone
    /// of the handle alive regardless of what the caller does with its
    /// copy. The returned token yields `(buffer, file)` once claimed.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_raw`], plus [`io::ErrorKind::InvalidInput`] if `file`
    /// is a [`RegisteredFile`] from a different ring.
    pub fn read<B: IoBufMut, F: FileTarget>(
        &mut self,
        file: &F,
        mut buffer: B,
        offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<(B, F::Guard)>> {
        self.require(Op::Read)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_mut_ptr().cast::<c_void>();
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let token = Token::new(self.ring, (buffer, file.guard()))?;
        let user_data = token.id();
        // SAFETY: `self.ring`'s handle is live; `address` is `IoBufMut`'s
        // promised stable, exclusively-owned pointer, valid for `len` bytes
        // until `token` is claimed; `target` stays valid at least that long
        // too, because `token` holds `file`'s guard -- a clone of the `Arc`
        // for a `SharedFile`, and nothing needing to be kept alive at all for
        // a `RegisteredFile`, whose index names the ring's own table.
        let hr = unsafe {
            BuildIoRingReadFile(
                self.ring.raw_handle(),
                target,
                raw_buffer_ref(address),
                len,
                offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// Queue a write of `buffer.bytes_len()` bytes to `file` at `offset`.
    ///
    /// `caching` is `FILE_WRITE_FLAGS`; see [`WriteCaching`], and note that
    /// write-through is a cache directive rather than a durability guarantee.
    /// Durability comes from [`Batch::flush`] with
    /// [`FlushCoverage::CoversPrecedingOperations`], never from a write flag.
    ///
    /// Prefer [`Batch::write`] unless `file` needs to address a raw
    /// `FileRef` directly, without `SharedFile`'s `Arc` bookkeeping.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_raw`]'s, for a write-access handle.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_raw`], plus any error from `BuildIoRingWriteFile`.
    pub unsafe fn write_raw<B: IoBuf>(
        &mut self,
        file: impl Into<FileRef>,
        buffer: B,
        offset: u64,
        options: PushOptions,
        caching: WriteCaching,
    ) -> io::Result<Token<B>> {
        self.require(Op::Write)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_ptr().cast_mut().cast::<c_void>();
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let token = Token::new(self.ring, buffer)?;
        let user_data = token.id();
        // SAFETY: `address` is `IoBuf`'s promised stable pointer, valid for
        // `len` bytes until `token` is claimed; the kernel only reads
        // through it for a write, so the cast away from `const` does not
        // authorize mutation. `file` is the caller's to keep alive.
        let hr = unsafe {
            BuildIoRingWriteFile(
                self.ring.raw_handle(),
                target,
                raw_buffer_ref(address),
                len,
                offset,
                caching.raw(),
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// As [`Batch::write_raw`], but safe: `file` is a [`SharedFile`] rather
    /// than a bare [`FileRef`]. The returned token yields `(buffer, file)`
    /// once claimed.
    ///
    /// # Errors
    ///
    /// As [`Batch::write_raw`], plus [`io::ErrorKind::InvalidInput`] if `file`
    /// is a [`RegisteredFile`] from a different ring.
    pub fn write<B: IoBuf, F: FileTarget>(
        &mut self,
        file: &F,
        buffer: B,
        offset: u64,
        options: PushOptions,
        caching: WriteCaching,
    ) -> io::Result<Token<(B, F::Guard)>> {
        self.require(Op::Write)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_ptr().cast_mut().cast::<c_void>();
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let token = Token::new(self.ring, (buffer, file.guard()))?;
        let user_data = token.id();
        // SAFETY: as `write_raw`'s; `target` stays valid at least as long as
        // `token`'s hold on `file`'s guard does (see `Batch::read`).
        let hr = unsafe {
            BuildIoRingWriteFile(
                self.ring.raw_handle(),
                target,
                raw_buffer_ref(address),
                len,
                offset,
                caching.raw(),
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// Refuse a [`RegisteredBuffers`] that did not come from this batch's own
    /// ring (PR #20 review response): its index space is only meaningful
    /// against the ring that registered it, and a different ring may have an
    /// entirely unrelated (or already-freed) buffer at the same index.
    fn check_registration_ring<B: IoBufMut>(
        &self,
        registration: &RegisteredBuffers<B>,
    ) -> io::Result<()> {
        if registration.ring_id == self.ring.ring_id() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "this RegisteredBuffers was registered on a different IoRing",
            ))
        }
    }

    /// Reclaim `token`'s value and release its reservation if `hr` failed,
    /// or hand `token` back unchanged on success -- the shared tail of every
    /// push in this module.
    fn finish_push<T: Send + 'static>(
        &mut self,
        hr: windows_sys::core::HRESULT,
        token: Token<T>,
    ) -> io::Result<Token<T>> {
        match check(hr) {
            Ok(()) => Ok(token),
            Err(error) => {
                // The SQE was never queued: reclaim and drop the value
                // normally instead of leaking it (this crate's own code
                // knows the op never reached the kernel, so an unconditional
                // `claim` is sound here -- unlike `claim_if`, which requires
                // a real popped `Completion`), and release the reservation
                // so it does not count against rundown.
                let _ = token.claim();
                self.ring.cancel_reservation();
                Err(error)
            }
        }
    }

    /// Queue a flush of `file`'s buffered data.
    ///
    /// `coverage` decides whether this flush covers the operations queued
    /// before it. It is required rather than defaulted because there is no
    /// safe default -- see [`FlushCoverage`] and the measured contract below.
    ///
    /// `mode` is `FILE_FLUSH_MODE`; [`FlushMode::Default`] is the durability
    /// barrier. Note that [`FlushMode::NoSync`] issues no device sync and so
    /// makes nothing durable, whatever `coverage` says.
    ///
    /// There is no buffer, so this returns the raw `UserData` identity
    /// rather than a [`Token`]: nothing owns a buffer for a completion to
    /// hand back. Prefer [`Batch::flush`] unless `file` needs to address a
    /// raw `FileRef` directly.
    ///
    /// # A flush is the ring's only durability primitive
    ///
    /// **The ring has no FUA.** `BuildIoRingWriteFile`'s entire flag set is
    /// `{FILE_WRITE_FLAGS_NONE, FILE_WRITE_FLAGS_WRITE_THROUGH}`, and
    /// write-through is a cache-bypass directive to the OS rather than a
    /// device-level durability guarantee -- whether it becomes a Force Unit
    /// Access bit depends on the driver, the volume, and whether the device's
    /// write cache is enabled (see [`WriteCaching`]). So this operation is the
    /// only way the ring makes anything durable, and only with
    /// [`FlushCoverage::CoversPrecedingOperations`] and a syncing
    /// [`FlushMode`].
    ///
    /// # The measured contract (D-23 in `DESIGN-NOTES.md`)
    ///
    /// **An unflagged flush does not cover preceding writes.** It is an
    /// ordinary operation competing with them, and it frequently wins: a
    /// flush pushed after a batch of writes with no barrier was observed
    /// completing while 17, and on another run 23, of 32 of those writes were
    /// still outstanding. A caller that reads such a completion as "the
    /// writes before it are now durable" has lost data it believes it has
    /// committed, and nothing reports the loss until power fails.
    ///
    /// Which *direction* the reordering shows in is device-dependent, and a
    /// machine where the flush happens to land last anyway proves nothing:
    /// that is incidental behavior of one device stack, not a guarantee. Only
    /// the barrier makes it one.
    ///
    /// Durability on this ring is therefore a property of an **epoch**, never
    /// of an individual write, because there is no per-write primitive to
    /// make it one: stream the writes unflagged, close the epoch with one
    /// covering flush, and wait on that flush rather than on the writes.
    /// "Durability on the ring" in `DESIGN-NOTES.md` has the full
    /// construction, and the three ways to pay for the barrier's ring-wide
    /// stall.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_raw`]'s, for a [`FileRef::Raw`] target.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the ring was not probed as
    /// supporting [`Op::Flush`]; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room; or any
    /// other error from `BuildIoRingFlushFile`.
    pub unsafe fn flush_raw(
        &mut self,
        file: impl Into<FileRef>,
        coverage: FlushCoverage,
        mode: FlushMode,
    ) -> io::Result<usize> {
        self.require(Op::Flush)?;
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let user_data = self.ring.reserve_user_data()?;
        // SAFETY: `self.ring`'s handle is live; `file` is the caller's to
        // keep alive, forwarded from this function's own contract; there is
        // no buffer.
        let hr = unsafe {
            BuildIoRingFlushFile(
                self.ring.raw_handle(),
                target,
                mode.raw(),
                user_data,
                coverage.sqe_flags(),
            )
        };
        if let Err(error) = check(hr) {
            self.ring.cancel_reservation();
            return Err(error);
        }
        Ok(user_data)
    }

    /// As [`Batch::flush_raw`], but safe: `file` is a [`FileTarget`] -- a
    /// [`SharedFile`] or a [`RegisteredFile`] -- and the returned [`Token`]
    /// (rather than a bare `UserData`) holds its guard until this
    /// operation's completion is observed.
    ///
    /// `coverage` is required for the reason [`FlushCoverage`] documents: an
    /// unflagged flush does not cover preceding writes (D-23), so there is no
    /// default spelling of this call that is safe to inherit. `mode` selects
    /// how much is flushed and whether the device is synced at all; see
    /// [`FlushMode`].
    ///
    /// # Errors
    ///
    /// As [`Batch::flush_raw`], plus [`io::ErrorKind::InvalidInput`] if
    /// `file` is a [`RegisteredFile`] from a different ring.
    pub fn flush<F: FileTarget>(
        &mut self,
        file: &F,
        coverage: FlushCoverage,
        mode: FlushMode,
    ) -> io::Result<Token<F::Guard>> {
        self.require(Op::Flush)?;
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let token = Token::new(self.ring, file.guard())?;
        let user_data = token.id();
        // SAFETY: `target` stays valid at least as long as `token`'s hold on
        // `file`'s guard does (see `Batch::read`); there is no buffer.
        let hr = unsafe {
            BuildIoRingFlushFile(
                self.ring.raw_handle(),
                target,
                mode.raw(),
                user_data,
                coverage.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// Queue cancellation of the operation identified by `target` (the
    /// `usize` a prior push returned), against `file`.
    ///
    /// A cancel is itself an operation: it completes on its own `UserData`,
    /// returned here, independently of whether `target` was actually
    /// outstanding. Cancelling a target that has already completed -- or
    /// was never outstanding -- reports `ERROR_NOT_FOUND` through *this*
    /// completion rather than failing to build (M3.6). Prefer
    /// [`Batch::cancel`] unless `file` needs to address a raw `FileRef`
    /// directly.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_raw`]'s, for a [`FileRef::Raw`] target.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the ring was not probed as
    /// supporting [`Op::Cancel`]; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room; or any
    /// other error from `BuildIoRingCancelRequest`.
    pub unsafe fn cancel_raw(
        &mut self,
        file: impl Into<FileRef>,
        target: usize,
    ) -> io::Result<usize> {
        self.require(Op::Cancel)?;
        let handle = handle_ref(file.into(), self.ring.ring_id())?;
        let user_data = self.ring.reserve_user_data()?;
        // SAFETY: `self.ring`'s handle is live; `file` is the caller's to
        // keep alive, forwarded from this function's own contract;
        // `BuildIoRingCancelRequest` takes no SQE-flags parameter.
        let hr =
            unsafe { BuildIoRingCancelRequest(self.ring.raw_handle(), handle, target, user_data) };
        if let Err(error) = check(hr) {
            self.ring.cancel_reservation();
            return Err(error);
        }
        Ok(user_data)
    }

    /// As [`Batch::cancel_raw`], but safe: `file` is a [`SharedFile`], and
    /// the returned [`Token`] keeps `file`'s clone alive until this
    /// operation's completion is observed.
    ///
    /// A cancel is a request, not a guarantee (M10.2): `target` may complete
    /// normally regardless, and this push produces its *own* completion in
    /// addition to the target's, so a cancelled operation yields two. A
    /// result of `ERROR_NOT_FOUND` on this push's own completion means
    /// `target` was no longer outstanding -- a normal race, not a caller
    /// error.
    ///
    /// # Errors
    ///
    /// As [`Batch::cancel_raw`].
    pub fn cancel<F: FileTarget>(
        &mut self,
        file: &F,
        target: usize,
    ) -> io::Result<Token<F::Guard>> {
        self.require(Op::Cancel)?;
        let handle = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let token = Token::new(self.ring, file.guard())?;
        let user_data = token.id();
        // SAFETY: `handle` stays valid at least as long as `token`'s hold on
        // `file`'s guard does (see `Batch::read`);
        // `BuildIoRingCancelRequest` takes no SQE-flags parameter.
        let hr =
            unsafe { BuildIoRingCancelRequest(self.ring.raw_handle(), handle, target, user_data) };
        self.finish_push(hr, token)
    }

    /// Queue registration of `handles` as a ring's file-handle table (M5.1).
    ///
    /// `BuildIoRingRegisterFileHandles` *replaces* the ring's entire
    /// file-handle table rather than appending to it (Win32 docs: "If a
    /// previous registration exists, this replaces the previous
    /// registration completely"), which would silently invalidate every
    /// [`RegisteredFile`] index a prior registration handed out. Rather than
    /// track and resubmit that whole prior table transparently, this method
    /// refuses a second registration outright: a ring accepts at most one
    /// file-handle registration *that assigned an index* in its lifetime.
    ///
    /// Two consequences of that rule being enforced against
    /// [`crate::IoRing::registered_file_count`] rather than a flag (M10.1):
    /// a zero-length `handles` does not spend the ring's one registration,
    /// since it hands out no index for a later replacement to invalidate;
    /// and the count advances when this call *queues*, not when its
    /// completion succeeds (D-14), so a registration whose completion
    /// reports failure has still spent it. There is no retry -- a consumer
    /// whose registration fails must build the registration on a new ring.
    ///
    /// `handles` only needs to stay valid for this call, unlike a data
    /// buffer referenced through an `IORING_HANDLE_REF`/`IORING_BUFFER_REF`:
    /// `BuildIoRingRegisterFileHandles` has no such ref, it takes the array
    /// directly and reads it synchronously -- confirmed by measurement, not
    /// assumed (D-32). The handles themselves must still stay open for as
    /// long as the registration is used -- this crate does not take
    /// ownership of them, only of their assigned indices' bookkeeping.
    ///
    /// Do **not** generalize this to [`Batch::register_buffers`]:
    /// `BuildIoRingRegisterBuffers` reads its array when the op *runs*, and
    /// assuming otherwise was a live use-after-free in 0.1.2.
    ///
    /// # Safety
    ///
    /// Every handle in `handles` must be valid, and must remain valid for
    /// as long as the resulting registration is used -- for the ring's
    /// remaining life, since Win32 has no unregister call (M8, PR #20
    /// review response). There is no safe counterpart: a single-push
    /// `Token` cannot express a lifetime spanning arbitrarily many later
    /// reads and writes against every registered index, unlike a `Token`
    /// tied to one push's own completion.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::AlreadyExists`] if this ring already has a
    /// file-handle registration (see above); [`io::ErrorKind::Unsupported`]
    /// if the ring was not probed as
    /// supporting [`Op::RegisterFiles`](crate::Op::RegisterFiles);
    /// [`io::ErrorKind::InvalidInput`] if `handles` has more than
    /// `u32::MAX` entries; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room; or any
    /// other error from `BuildIoRingRegisterFileHandles`.
    pub unsafe fn register_files(
        &mut self,
        handles: &[HANDLE],
    ) -> io::Result<PendingFileRegistration> {
        self.require(Op::RegisterFiles)?;
        if self.ring.registered_file_count() > 0 {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "this ring already has a file-handle registration; BuildIoRingRegisterFileHandles \
                 replaces the whole table, so a second call would invalidate every RegisteredFile \
                 index already handed out",
            ));
        }
        let count = checked_len(handles.len())?;
        let base_index = self.ring.registered_file_count();
        let user_data = self.ring.reserve_user_data()?;
        // SAFETY: `self.ring`'s handle is live; `handles` is read
        // synchronously for the duration of this call only -- confirmed by
        // measurement (D-32), not inherited from the sibling registration,
        // which behaves the opposite way.
        let hr = unsafe {
            BuildIoRingRegisterFileHandles(
                self.ring.raw_handle(),
                count,
                handles.as_ptr(),
                user_data,
            )
        };
        if let Err(error) = check(hr) {
            self.ring.cancel_reservation();
            return Err(error);
        }
        self.ring.reserve_registered_files(count);
        Ok(PendingFileRegistration {
            user_data,
            base_index,
            count,
            ring_id: self.ring.ring_id(),
        })
    }

    /// Queue registration of `buffers` as a ring's registered-buffer table
    /// (M5.2).
    ///
    /// As [`Batch::register_files`]: `BuildIoRingRegisterBuffers` replaces
    /// the ring's entire buffer table rather than appending to it, so this
    /// method refuses a second registration outright -- a ring accepts at
    /// most one buffer registration *that assigned an index* in its
    /// lifetime, with the same zero-length and failed-registration
    /// consequences [`Batch::register_files`] spells out (M10.1).
    ///
    /// Unlike [`Batch::register_files`], **two** things must outlive this
    /// call, and the asymmetry is measured rather than assumed (D-32):
    ///
    /// - the *bytes each entry points at* -- the registration case `IoBuf`'s
    ///   contract was extended to cover (D-11), so `buffers` is taken by
    ///   value and kept inside the returned [`RegisteredBuffers`] once
    ///   claimed;
    /// - the `IORING_BUFFER_INFO` array itself. `BuildIoRingRegisterBuffers`
    ///   does **not** read it synchronously the way
    ///   `BuildIoRingRegisterFileHandles` reads its `handles` array; the
    ///   kernel reads it when the registration op runs, during a later
    ///   `SubmitIoRing`. This crate builds that array and hands it to the
    ///   ring, which holds it for its remaining life, so a caller has
    ///   nothing to do -- but the distinction is why
    ///   [`crate::IoRing::push_raw`] callers building this op themselves must
    ///   not pass a temporary.
    ///
    /// # Errors
    ///
    /// As [`Batch::register_files`], for [`Op::RegisterBuffers`](crate::Op::RegisterBuffers)
    /// and `BuildIoRingRegisterBuffers`.
    pub fn register_buffers<B: IoBufMut>(
        &mut self,
        mut buffers: Vec<B>,
    ) -> io::Result<PendingBufferRegistration<B>> {
        self.require(Op::RegisterBuffers)?;
        if self.ring.registered_buffer_count() > 0 {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "this ring already has a buffer registration; BuildIoRingRegisterBuffers \
                 replaces the whole table, so a second call would invalidate every buffer \
                 index already handed out",
            ));
        }
        let count = checked_len(buffers.len())?;
        let base_index = self.ring.registered_buffer_count();
        let mut infos = Vec::with_capacity(buffers.len());
        for buffer in &mut buffers {
            let length = checked_len(buffer.bytes_len())?;
            infos.push(IORING_BUFFER_INFO {
                Address: buffer.stable_mut_ptr().cast::<c_void>(),
                Length: length,
            });
        }
        let user_data = self.ring.reserve_user_data()?;
        // The array must outlive this call: the kernel reads it when the
        // registration op runs, during a later `SubmitIoRing`, not here
        // (D-32, measured). Hand it to the ring, which holds it for its
        // remaining life, and build the SQE from *that* pointer rather than
        // from the local `Vec` about to go out of scope.
        let infos_ptr = self.ring.hold_registered_buffer_infos(infos);
        // SAFETY: `self.ring`'s handle is live; `infos_ptr` addresses the
        // array the ring now owns, which outlives every submit that could run
        // this SQE; each `Address` points into `buffers`, which the caller
        // keeps alive via the returned `PendingBufferRegistration` and, once
        // claimed, `RegisteredBuffers`.
        let hr = unsafe {
            BuildIoRingRegisterBuffers(self.ring.raw_handle(), count, infos_ptr, user_data)
        };
        if let Err(error) = check(hr) {
            self.ring.cancel_reservation();
            return Err(error);
        }
        self.ring.reserve_registered_buffers(count);
        Ok(PendingBufferRegistration {
            user_data,
            base_index,
            buffers: ManuallyDrop::new(buffers),
            ring_id: self.ring.ring_id(),
        })
    }

    /// Queue a read of `span.len` bytes from `file` at `file_offset`, into
    /// `span`'s byte offset of `registration`'s buffer at `span.buffer_index`,
    /// instead of handing over a fresh owned buffer (M5.2).
    ///
    /// The returned [`Token`] must be claimed once its completion is
    /// observed, exactly like [`Batch::read`]'s -- but claiming it recovers
    /// no buffer, only releases this use against `registration`'s own drop
    /// check (M5.3). Read the transferred bytes back from `registration`
    /// itself afterward, for example via a caller-side accessor into the
    /// buffer it was constructed from. Prefer [`Batch::read_registered`]
    /// unless `file` needs to address a raw `FileRef` directly.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_raw`]'s.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_raw`], plus [`io::ErrorKind::InvalidInput`] if
    /// `span.buffer_index` is out of range for `registration`.
    pub unsafe fn read_registered_raw<B: IoBufMut>(
        &mut self,
        file: impl Into<FileRef>,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        file_offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<RegisteredUse>> {
        self.require(Op::Read)?;
        self.check_registration_ring(registration)?;
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        registration.outstanding.fetch_add(1, Ordering::SeqCst);
        let token = Token::new(
            self.ring,
            RegisteredUse(Arc::clone(&registration.outstanding)),
        )?;
        let user_data = token.id();
        // SAFETY: `self.ring`'s handle is live; `index` was just checked
        // against `registration`, whose buffer stays put until it drops;
        // `file` is the caller's to keep alive, forwarded from this
        // function's own contract.
        let hr = unsafe {
            BuildIoRingReadFile(
                self.ring.raw_handle(),
                target,
                registered_buffer_ref(index, span.offset),
                span.len,
                file_offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// As [`Batch::read_registered_raw`], but safe: `file` is a
    /// [`FileTarget`] rather than a bare [`FileRef`]. The returned token
    /// yields `(RegisteredUse, guard)` once claimed.
    ///
    /// Passing a [`RegisteredFile`] here is the fully-registered form --
    /// registered file *and* registered buffer, neither costing a handle
    /// lookup nor a buffer pin per operation -- which before M10.4 the safe
    /// API could not express at all.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_registered_raw`], plus
    /// [`io::ErrorKind::InvalidInput`] if `file` is a [`RegisteredFile`] from
    /// a different ring.
    pub fn read_registered<B: IoBufMut, F: FileTarget>(
        &mut self,
        file: &F,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        file_offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<(RegisteredUse, F::Guard)>> {
        self.require(Op::Read)?;
        self.check_registration_ring(registration)?;
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        registration.outstanding.fetch_add(1, Ordering::SeqCst);
        let token = Token::new(
            self.ring,
            (
                RegisteredUse(Arc::clone(&registration.outstanding)),
                file.guard(),
            ),
        )?;
        let user_data = token.id();
        // SAFETY: `index` was just checked against `registration`, whose
        // buffer stays put until it drops; `target` stays valid at least as
        // long as `token`'s hold on `file`'s guard does (see `Batch::read`).
        let hr = unsafe {
            BuildIoRingReadFile(
                self.ring.raw_handle(),
                target,
                registered_buffer_ref(index, span.offset),
                span.len,
                file_offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// Queue a write of `span.len` bytes to `file` at `file_offset`, from
    /// `span`'s byte offset of `registration`'s buffer at `span.buffer_index`
    /// (M5.2).
    ///
    /// As [`Batch::read_registered_raw`], but for `BuildIoRingWriteFile`.
    /// Prefer [`Batch::write_registered`] unless `file` needs to address a
    /// raw `FileRef` directly.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_registered_raw`]'s.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_registered_raw`].
    pub unsafe fn write_registered_raw<B: IoBufMut>(
        &mut self,
        file: impl Into<FileRef>,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        file_offset: u64,
        options: PushOptions,
        caching: WriteCaching,
    ) -> io::Result<Token<RegisteredUse>> {
        self.require(Op::Write)?;
        self.check_registration_ring(registration)?;
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        registration.outstanding.fetch_add(1, Ordering::SeqCst);
        let token = Token::new(
            self.ring,
            RegisteredUse(Arc::clone(&registration.outstanding)),
        )?;
        let user_data = token.id();
        // SAFETY: as `read_registered_raw`; the kernel only reads through
        // this reference for a write.
        let hr = unsafe {
            BuildIoRingWriteFile(
                self.ring.raw_handle(),
                target,
                registered_buffer_ref(index, span.offset),
                span.len,
                file_offset,
                caching.raw(),
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// As [`Batch::write_registered_raw`], but safe: `file` is a
    /// [`FileTarget`] rather than a bare [`FileRef`]. The returned token
    /// yields `(RegisteredUse, guard)` once claimed.
    ///
    /// Passing a [`RegisteredFile`] here is the fully-registered form, as
    /// for [`Batch::read_registered`].
    ///
    /// # Errors
    ///
    /// As [`Batch::write_registered_raw`], plus
    /// [`io::ErrorKind::InvalidInput`] if `file` is a [`RegisteredFile`] from
    /// a different ring.
    pub fn write_registered<B: IoBufMut, F: FileTarget>(
        &mut self,
        file: &F,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        file_offset: u64,
        options: PushOptions,
        caching: WriteCaching,
    ) -> io::Result<Token<(RegisteredUse, F::Guard)>> {
        self.require(Op::Write)?;
        self.check_registration_ring(registration)?;
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        registration.outstanding.fetch_add(1, Ordering::SeqCst);
        let token = Token::new(
            self.ring,
            (
                RegisteredUse(Arc::clone(&registration.outstanding)),
                file.guard(),
            ),
        )?;
        let user_data = token.id();
        // SAFETY: as `read_registered`'s; the kernel only reads through
        // this reference for a write.
        let hr = unsafe {
            BuildIoRingWriteFile(
                self.ring.raw_handle(),
                target,
                registered_buffer_ref(index, span.offset),
                span.len,
                file_offset,
                caching.raw(),
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// Submit everything queued so far, returning the number of entries the
    /// kernel accepted.
    ///
    /// That count is entries *submitted*, never entries completed (M10.2):
    /// draining is [`crate::IoRing::try_pop`]'s job and stays the caller's.
    ///
    /// # Errors
    ///
    /// Returns any error from `SubmitIoRing`.
    pub fn submit(self) -> io::Result<u32> {
        self.submit_and_wait(0, 0)
    }

    /// Submit everything queued so far, then block until at least
    /// `wait_operations` of them have completed or `timeout_ms` elapses
    /// (D-3's Model B primitive: the fused submit-and-wait *is* the event
    /// loop for a pinned-thread consumer).
    ///
    /// Returning does not mean `wait_operations` completions are poppable:
    /// the timeout can expire first, and the returned count is entries
    /// submitted rather than completed (M10.2). Drain with
    /// [`crate::IoRing::try_pop`] and count for yourself.
    ///
    /// # Errors
    ///
    /// Returns any error from `SubmitIoRing`.
    pub fn submit_and_wait(mut self, wait_operations: u32, timeout_ms: u32) -> io::Result<u32> {
        self.do_submit(wait_operations, timeout_ms)
    }

    fn do_submit(&mut self, wait_operations: u32, timeout_ms: u32) -> io::Result<u32> {
        let mut submitted = 0_u32;
        // SAFETY: `self.ring`'s handle is live.
        let hr = unsafe {
            SubmitIoRing(
                self.ring.raw_handle(),
                wait_operations,
                timeout_ms,
                &raw mut submitted,
            )
        };
        // Marked attempted *before* propagating a failure (PR #20 review
        // response): `submit_and_wait` takes `self` by value, so this call
        // failing still runs `Drop` once its caller's `self` goes out of
        // scope. Setting this first is what stops `Drop` from silently
        // retrying an explicit submit that already ran -- which could
        // succeed on the retry, submitting operations the caller's `Err`
        // never told them about.
        self.submitted = true;
        check(hr)?;
        Ok(submitted)
    }
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        if !self.submitted {
            // Best-effort: Drop cannot propagate an error, and a batch that
            // queued nothing submits zero entries harmlessly (D-5).
            let _ = self.do_submit(0, 0);
        }
    }
}

#[cfg(test)]
mod tests;
