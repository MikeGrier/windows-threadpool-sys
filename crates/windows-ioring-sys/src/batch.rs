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
    FILE_FLUSH_DEFAULT, FILE_WRITE_FLAGS_NONE, IORING_BUFFER_INFO, IORING_BUFFER_REF,
    IORING_BUFFER_REF_0, IORING_HANDLE_REF, IORING_HANDLE_REF_0, IORING_REF_RAW,
    IORING_REF_REGISTERED, IORING_REGISTERED_BUFFER, IORING_SQE_FLAGS,
    IOSQE_FLAGS_DRAIN_PRECEDING_OPS, IOSQE_FLAGS_NONE, SubmitIoRing,
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
    /// every op already queued in this batch's ring has completed. A
    /// barrier, not a cheap tag -- it forces the ring to drain before
    /// continuing.
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

/// The confirmed result of a [`Batch::register_files`] push: `count`
/// contiguous indices starting at `base_index`.
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
    /// As [`Batch::read_raw`].
    pub fn read<B: IoBufMut>(
        &mut self,
        file: &SharedFile,
        mut buffer: B,
        offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<(B, SharedFile)>> {
        self.require(Op::Read)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_mut_ptr().cast::<c_void>();
        let raw = file.raw_handle();
        let target = handle_ref(FileRef::Raw(raw), self.ring.ring_id())?;
        let token = Token::new(self.ring, (buffer, file.clone()))?;
        let user_data = token.id();
        // SAFETY: `self.ring`'s handle is live; `address` is `IoBufMut`'s
        // promised stable, exclusively-owned pointer, valid for `len` bytes
        // until `token` is claimed; `raw` stays valid at least that long
        // too, since `token` holds its own clone of `file`'s `Arc`.
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
                FILE_WRITE_FLAGS_NONE,
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
    /// As [`Batch::write_raw`].
    pub fn write<B: IoBuf>(
        &mut self,
        file: &SharedFile,
        buffer: B,
        offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<(B, SharedFile)>> {
        self.require(Op::Write)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_ptr().cast_mut().cast::<c_void>();
        let raw = file.raw_handle();
        let target = handle_ref(FileRef::Raw(raw), self.ring.ring_id())?;
        let token = Token::new(self.ring, (buffer, file.clone()))?;
        let user_data = token.id();
        // SAFETY: as `write_raw`'s; `raw` stays valid at least as long as
        // `token`'s own clone of `file`'s `Arc` does.
        let hr = unsafe {
            BuildIoRingWriteFile(
                self.ring.raw_handle(),
                target,
                raw_buffer_ref(address),
                len,
                offset,
                FILE_WRITE_FLAGS_NONE,
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

    /// Queue a flush of `file`'s buffered data (`FILE_FLUSH_DEFAULT`).
    ///
    /// There is no buffer, so this returns the raw `UserData` identity
    /// rather than a [`Token`]: nothing owns a buffer for a completion to
    /// hand back. Prefer [`Batch::flush`] unless `file` needs to address a
    /// raw `FileRef` directly.
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
        options: PushOptions,
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
                FILE_FLUSH_DEFAULT,
                user_data,
                options.sqe_flags(),
            )
        };
        if let Err(error) = check(hr) {
            self.ring.cancel_reservation();
            return Err(error);
        }
        Ok(user_data)
    }

    /// As [`Batch::flush_raw`], but safe: `file` is a [`SharedFile`], and
    /// the returned [`Token`] (rather than a bare `UserData`) keeps
    /// `file`'s clone alive until this operation's completion is observed.
    ///
    /// # Errors
    ///
    /// As [`Batch::flush_raw`].
    pub fn flush(
        &mut self,
        file: &SharedFile,
        options: PushOptions,
    ) -> io::Result<Token<SharedFile>> {
        self.require(Op::Flush)?;
        let raw = file.raw_handle();
        let target = handle_ref(FileRef::Raw(raw), self.ring.ring_id())?;
        let token = Token::new(self.ring, file.clone())?;
        let user_data = token.id();
        // SAFETY: `raw` stays valid at least as long as `token`'s own clone
        // of `file`'s `Arc` does; there is no buffer.
        let hr = unsafe {
            BuildIoRingFlushFile(
                self.ring.raw_handle(),
                target,
                FILE_FLUSH_DEFAULT,
                user_data,
                options.sqe_flags(),
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
    /// # Errors
    ///
    /// As [`Batch::cancel_raw`].
    pub fn cancel(&mut self, file: &SharedFile, target: usize) -> io::Result<Token<SharedFile>> {
        self.require(Op::Cancel)?;
        let raw = file.raw_handle();
        let handle = handle_ref(FileRef::Raw(raw), self.ring.ring_id())?;
        let token = Token::new(self.ring, file.clone())?;
        let user_data = token.id();
        // SAFETY: `raw` stays valid at least as long as `token`'s own clone
        // of `file`'s `Arc` does; `BuildIoRingCancelRequest` takes no
        // SQE-flags parameter.
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
    /// directly and reads it synchronously. The handles themselves must
    /// still stay open for as long as the registration is used -- this
    /// crate does not take ownership of them, only of their assigned
    /// indices' bookkeeping.
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
        // synchronously for the duration of this call only.
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
    /// Unlike [`Batch::register_files`], what must outlive this call is not
    /// the array `BuildIoRingRegisterBuffers` reads (also synchronous, also
    /// a bare pointer with no ref indirection) but the *bytes each entry
    /// points at* -- this is exactly the registration case `IoBuf`'s
    /// contract was extended to cover (D-11), so `buffers` is taken by
    /// value and kept inside the returned [`RegisteredBuffers`] once claimed.
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
        // SAFETY: `self.ring`'s handle is live; `infos` is read synchronously
        // for the duration of this call; each `Address` points into
        // `buffers`, which the caller keeps alive via the returned
        // `PendingBufferRegistration` and, once claimed, `RegisteredBuffers`.
        let hr = unsafe {
            BuildIoRingRegisterBuffers(self.ring.raw_handle(), count, infos.as_ptr(), user_data)
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
    /// [`SharedFile`] rather than a bare [`FileRef`]. The returned token
    /// yields `(RegisteredUse, file)` once claimed.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_registered_raw`].
    pub fn read_registered<B: IoBufMut>(
        &mut self,
        file: &SharedFile,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        file_offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<(RegisteredUse, SharedFile)>> {
        self.require(Op::Read)?;
        self.check_registration_ring(registration)?;
        let raw = file.raw_handle();
        let target = handle_ref(FileRef::Raw(raw), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        registration.outstanding.fetch_add(1, Ordering::SeqCst);
        let token = Token::new(
            self.ring,
            (
                RegisteredUse(Arc::clone(&registration.outstanding)),
                file.clone(),
            ),
        )?;
        let user_data = token.id();
        // SAFETY: `index` was just checked against `registration`, whose
        // buffer stays put until it drops; `raw` stays valid at least as
        // long as `token`'s own clone of `file`'s `Arc` does.
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
                FILE_WRITE_FLAGS_NONE,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// As [`Batch::write_registered_raw`], but safe: `file` is a
    /// [`SharedFile`] rather than a bare [`FileRef`]. The returned token
    /// yields `(RegisteredUse, file)` once claimed.
    ///
    /// # Errors
    ///
    /// As [`Batch::write_registered_raw`].
    pub fn write_registered<B: IoBufMut>(
        &mut self,
        file: &SharedFile,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        file_offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<(RegisteredUse, SharedFile)>> {
        self.require(Op::Write)?;
        self.check_registration_ring(registration)?;
        let raw = file.raw_handle();
        let target = handle_ref(FileRef::Raw(raw), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        registration.outstanding.fetch_add(1, Ordering::SeqCst);
        let token = Token::new(
            self.ring,
            (
                RegisteredUse(Arc::clone(&registration.outstanding)),
                file.clone(),
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
                FILE_WRITE_FLAGS_NONE,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token)
    }

    /// Submit everything queued so far, returning the number of entries the
    /// kernel accepted.
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
