// Copyright (c) 2026 Mike Grier
// Split from batch.rs at f6e92aa7.
//! The push surface: the calls that queue one operation onto a ring.
//!
//! Separated from [`super`], which keeps the type definitions, the private
//! plumbing every push shares (`require`, `begin_owned`, `finish_owned`,
//! `check_registration_ring`) and the submit path.
//!
//! The cut runs this way round because Rust's privacy is asymmetric: a child
//! module can see its ancestors' private items, a parent cannot see its
//! children's. Moving the *definitions* down would have broken the parent's
//! use of `PushOptions::sqe_flags`, `WriteCaching::raw` and the rest, all of
//! which are private; moving the *consumers* down costs nothing.

use super::*;

impl<'ring, T, X> Batch<'ring, T, X> {
    /// Queue a write of `buffer.bytes_len()` bytes to `file` at `offset`,
    /// whose buffer the **ring** holds (`D-71`, `D-73`).
    ///
    /// The raw counterpart to [`Batch::write_owned`]: prefer that unless `file`
    /// needs to address a raw `FileRef` directly, without `SharedFile`'s `Arc`
    /// bookkeeping.
    ///
    /// # Safety
    ///
    /// If `file` is [`FileRef::Raw`], the handle must be valid, opened with
    /// write access, and must remain valid -- not closed, not reused for a
    /// different object -- until this operation's completion is observed (via a
    /// popped [`Completion`]) or until the ring runs down (M8, PR #20 review
    /// response). A [`FileRef::Registered`] target needs none of this.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the ring was not probed as supporting
    /// [`Op::Write`]; [`io::ErrorKind::InvalidInput`] if the buffer is longer
    /// than `u32::MAX`; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room (M3.3, not
    /// auto-flushed -- see [`Batch`]'s own docs); or any other error from
    /// `BuildIoRingWriteFile`. On any error the buffer is dropped normally,
    /// not leaked or handed back.
    pub unsafe fn write_raw_owned(
        &mut self,
        file: impl Into<FileRef>,
        buffer: T,
        extra: X,
        offset: u64,
        options: PushOptions,
        caching: WriteCaching,
    ) -> io::Result<OperationId>
    where
        T: IoBuf,
    {
        self.require(Op::Write)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_ptr().cast::<c_void>().cast_mut();
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let (user_data, id) = self.begin_owned()?;
        // SAFETY: as `write_raw` -- `address` is `IoBuf`'s promised stable
        // pointer, valid for `len` bytes, and stays valid across the move into
        // the inventory because that stability is the trait's contract rather
        // than a property of where the value lives.
        let hr = unsafe {
            crate::sys::build_write(
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
        self.finish_owned(hr, id, Some(buffer), extra, Held::default())
    }

    /// Queue a read against a guarded file, with the **ring** holding both the
    /// buffer and the guard (`D-73`).
    ///
    /// The guarded counterpart to [`Batch::read_raw_owned`]. The guard goes into
    /// the ring's own slot rather than into `T`, which is what keeps the
    /// caller's parameters free of this crate's internals.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_raw_owned`].
    pub fn read_owned<F: FileTarget>(
        &mut self,
        file: &F,
        mut buffer: T,
        extra: X,
        offset: u64,
        options: PushOptions,
    ) -> io::Result<OperationId>
    where
        T: IoBufMut,
    {
        self.require(Op::Read)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_mut_ptr().cast::<c_void>();
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let (user_data, id) = self.begin_owned()?;
        let held = Held {
            guard: Some(file.guard().into()),
            registration: None,
        };
        // SAFETY: as `read` -- `target` stays valid at least as long as the
        // operation, because the ring holds `file`'s guard until the pop that
        // completes it.
        let hr = unsafe {
            crate::sys::build_read(
                self.ring.raw_handle(),
                target,
                raw_buffer_ref(address),
                len,
                offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_owned(hr, id, Some(buffer), extra, held)
    }

    /// Queue a write against a guarded file, with the **ring** holding both
    /// the buffer and the guard (`D-73`).
    ///
    /// The guarded counterpart to [`Batch::write_raw_owned`].
    ///
    /// # Errors
    ///
    /// As [`Batch::write_raw_owned`].
    pub fn write_owned<F: FileTarget>(
        &mut self,
        file: &F,
        buffer: T,
        extra: X,
        offset: u64,
        options: PushOptions,
        caching: WriteCaching,
    ) -> io::Result<OperationId>
    where
        T: IoBuf,
    {
        self.require(Op::Write)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_ptr().cast::<c_void>().cast_mut();
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let (user_data, id) = self.begin_owned()?;
        let held = Held {
            guard: Some(file.guard().into()),
            registration: None,
        };
        // SAFETY: as `write`.
        let hr = unsafe {
            crate::sys::build_write(
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
        self.finish_owned(hr, id, Some(buffer), extra, held)
    }

    /// Queue a read whose buffer the **ring** holds, returning the operation's
    /// name (`D-71`, `D-73`).
    ///
    /// The raw counterpart to [`Batch::read_owned`]: prefer that unless `file`
    /// needs to address a raw `FileRef` directly, without `SharedFile`'s `Arc`
    /// bookkeeping. The caller hands over the buffer and its sidecar and
    /// receives an [`OperationId`], which names the operation and grants
    /// nothing; the buffer comes back from [`IoRing::try_pop`] and from nowhere
    /// else. A consumer that never holds a token cannot lose one, which is the
    /// whole of `D-55`.
    ///
    /// # Safety
    ///
    /// If `file` is [`FileRef::Raw`], the handle must be valid, opened with
    /// read access, and must remain valid -- not closed, not reused for a
    /// different object -- until this operation's completion is observed (via a
    /// popped [`Completion`]) or until the ring runs down (M8, PR #20 review
    /// response). A [`FileRef::Registered`] target needs none of this.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the ring was not probed as supporting
    /// [`Op::Read`]; [`io::ErrorKind::InvalidInput`] if the buffer is longer
    /// than `u32::MAX`; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room (M3.3, not
    /// auto-flushed -- see [`Batch`]'s own docs); or any other error from
    /// `BuildIoRingReadFile`. On any error the buffer is **dropped normally**,
    /// not leaked and not handed back: the return carries only an
    /// [`io::Error`], and a `Build*` that failed queued no SQE, so nothing will
    /// ever complete to reclaim it.
    pub unsafe fn read_raw_owned(
        &mut self,
        file: impl Into<FileRef>,
        mut buffer: T,
        extra: X,
        offset: u64,
        options: PushOptions,
    ) -> io::Result<OperationId>
    where
        T: IoBufMut,
    {
        self.require(Op::Read)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_mut_ptr().cast::<c_void>();
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let (user_data, id) = self.begin_owned()?;
        // SAFETY: as `read_raw` -- `address` is `IoBufMut`'s promised stable
        // pointer, valid for `len` bytes, and it stays valid across the move
        // into the inventory below because that stability is the trait's
        // contract rather than a property of where the value lives.
        let hr = unsafe {
            crate::sys::build_read(
                self.ring.raw_handle(),
                target,
                raw_buffer_ref(address),
                len,
                offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_owned(hr, id, Some(buffer), extra, Held::default())
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
    /// rather than an inventory entry: nothing owns a buffer for a completion to
    /// hand back. Prefer [`Batch::flush_owned`] unless `file` needs to address a
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
    /// wait.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_raw_owned`]'s, for a [`FileRef::Raw`] target.
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
            crate::sys::build_flush(
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

    /// Queue a raw flush, with the **ring** holding the sidecar (`D-73`).
    ///
    /// The inventory counterpart to [`Batch::flush_raw`]. There is no buffer
    /// and no guard -- a raw handle is the caller's to keep alive, exactly as
    /// for [`Batch::flush_raw`] -- so the entry exists purely to carry
    /// `extra`. That is not a degenerate case: a flush that cannot say which
    /// group of writes it belongs to forces the caller back into the
    /// side-table this API exists to remove.
    ///
    /// # Safety
    ///
    /// As [`Batch::flush_raw`].
    ///
    /// # Errors
    ///
    /// As [`Batch::flush_raw`].
    pub unsafe fn flush_raw_owned(
        &mut self,
        file: impl Into<FileRef>,
        extra: X,
        coverage: FlushCoverage,
        mode: FlushMode,
    ) -> io::Result<OperationId> {
        self.require(Op::Flush)?;
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let (user_data, id) = self.begin_owned()?;
        // SAFETY: as `flush_raw` -- `file` is the caller's to keep alive,
        // forwarded from this function's own contract; there is no buffer.
        let hr = unsafe {
            crate::sys::build_flush(
                self.ring.raw_handle(),
                target,
                mode.raw(),
                user_data,
                coverage.sqe_flags(),
            )
        };
        self.finish_owned(
            hr,
            id,
            None,
            extra,
            Held {
                guard: None,
                registration: None,
            },
        )
    }

    /// Queue a flush, with the **ring** holding the file guard (`D-73`).
    ///
    /// The guarded counterpart to [`Batch::flush_raw_owned`]. There is no
    /// buffer, so nothing comes back as a payload -- the pop yields `None` for
    /// it, which is the shape `M28.5` will settle for the tokenless pushes
    /// generally. The guard still has to outlive the operation, and the ring is
    /// what holds it.
    ///
    /// # Errors
    ///
    /// As [`Batch::flush_raw_owned`].
    pub fn flush_owned<F: FileTarget>(
        &mut self,
        file: &F,
        extra: X,
        coverage: FlushCoverage,
        mode: FlushMode,
    ) -> io::Result<OperationId> {
        self.require(Op::Flush)?;
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let (user_data, id) = self.begin_owned()?;
        let held = Held {
            guard: Some(file.guard().into()),
            registration: None,
        };
        // SAFETY: as `flush` -- `target` stays valid at least as long as the
        // ring's hold on `file`'s guard, which lasts until the pop that
        // completes this operation.
        let hr = unsafe {
            crate::sys::build_flush(
                self.ring.raw_handle(),
                target,
                mode.raw(),
                user_data,
                coverage.sqe_flags(),
            )
        };
        self.finish_owned(hr, id, None, extra, held)
    }

    /// Queue a cancellation of `target`, with the **ring** holding the file
    /// guard (`D-73`).
    ///
    /// The inventory counterpart to [`Batch::cancel_raw`].
    ///
    /// `target` is an [`OperationId`] rather than a bare `UserData`, and this
    /// checks that **this** ring minted it. Every ring hands out `UserData`
    /// from its own counter starting at the same value, so one integer
    /// legitimately names a different operation on each ring -- which is why an
    /// [`OperationId`] carries the ring that minted it alongside the
    /// `UserData`, and why that half must not be discarded. Accepting the bare
    /// integer
    /// would therefore let an identity from one ring cancel an unrelated
    /// operation on another, silently, and most readily when both address the
    /// same file. This is the same check [`Batch`] already applies to a
    /// [`RegisteredFile`] belonging to the wrong ring.
    ///
    /// To cancel a `UserData` this ring never minted an [`OperationId`] for,
    /// use [`Batch::cancel_owned_raw`], which says so at the call site.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidInput`] if `target` was minted by a different
    /// ring; otherwise as [`Batch::cancel_raw`].
    pub fn cancel_owned<F: FileTarget>(
        &mut self,
        file: &F,
        target: OperationId,
        extra: X,
    ) -> io::Result<OperationId> {
        if target.ring_id() != self.ring.ring_id() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "this OperationId was minted by a different IoRing",
            ));
        }
        self.cancel_owned_raw(file, target.user_data(), extra)
    }

    /// [`Batch::cancel_owned`] against a raw `UserData`, with no ring check.
    ///
    /// For the targets an [`OperationId`] cannot express: a `UserData` observed
    /// out of band, or -- as this crate's own tests use it -- one chosen
    /// precisely *because* nothing is outstanding under it, to exercise the
    /// `ERROR_NOT_FOUND` the kernel reports through the cancel's own
    /// completion.
    ///
    /// Safe rather than `unsafe`, because a cancel returns no memory: naming
    /// the wrong operation costs a wrong cancellation, not a use-after-free.
    /// It is a separate method so that cost is visible at the call site instead
    /// of hidden inside an integer conversion.
    ///
    /// # Errors
    ///
    /// As [`Batch::cancel_raw`].
    pub fn cancel_owned_raw<F: FileTarget>(
        &mut self,
        file: &F,
        target: usize,
        extra: X,
    ) -> io::Result<OperationId> {
        self.require(Op::Cancel)?;
        let handle = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let (user_data, id) = self.begin_owned()?;
        let held = Held {
            guard: Some(file.guard().into()),
            registration: None,
        };
        // SAFETY: as `cancel_raw`.
        let hr =
            unsafe { crate::sys::build_cancel(self.ring.raw_handle(), handle, target, user_data) };
        self.finish_owned(hr, id, None, extra, held)
    }

    /// Queue cancellation of the operation identified by `target` (the
    /// `usize` a prior push returned), against `file`.
    ///
    /// A cancel is itself an operation: it completes on its own `UserData`,
    /// returned here, independently of whether `target` was actually
    /// outstanding. Cancelling a target that has already completed -- or
    /// was never outstanding -- reports `ERROR_NOT_FOUND` through *this*
    /// completion rather than failing to build (M3.6). Prefer
    /// [`Batch::cancel_owned`] unless `file` needs to address a raw `FileRef`
    /// directly.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_raw_owned`]'s, for a [`FileRef::Raw`] target.
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
            unsafe { crate::sys::build_cancel(self.ring.raw_handle(), handle, target, user_data) };
        if let Err(error) = check(hr) {
            self.ring.cancel_reservation();
            return Err(error);
        }
        Ok(user_data)
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
            crate::sys::build_register_files(
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
        let mut registered_lens = Vec::with_capacity(buffers.len());
        for buffer in &mut buffers {
            let length = checked_len(buffer.bytes_len())?;
            registered_lens.push(length);
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
            crate::sys::build_register_buffers(self.ring.raw_handle(), count, infos_ptr, user_data)
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
            registered_lens,
            ring_id: self.ring.ring_id(),
        })
    }

    /// Queue a read of `span.len` bytes from `file` at `file_offset`, into
    /// `span`'s byte offset of `registration`'s buffer at `span.buffer_index`,
    /// instead of handing over a fresh owned buffer (M5.2), with the **ring**
    /// holding the registration's use count (`D-73`).
    ///
    /// The raw counterpart to [`Batch::read_registered_owned`]: prefer that
    /// unless `file` needs to address a raw `FileRef` directly.
    ///
    /// The buffer belongs to the registration rather than to the caller, so
    /// there is no payload to give back -- what the ring holds is the *use*,
    /// which keeps the registration from being torn down while the kernel is
    /// writing into it. That is `Held.registration`, released at the pop that
    /// completes this operation (M5.3). Read the transferred bytes back from
    /// `registration` itself afterward, for example via a caller-side accessor
    /// into the buffer it was constructed from.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_raw_owned`]'s.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_raw_owned`], plus [`io::ErrorKind::InvalidInput`] if
    /// `span.buffer_index` is out of range for `registration`.
    pub unsafe fn read_registered_raw_owned<B: IoBufMut>(
        &mut self,
        file: impl Into<FileRef>,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        extra: X,
        file_offset: u64,
        options: PushOptions,
    ) -> io::Result<OperationId> {
        self.require(Op::Read)?;
        self.check_registration_ring(registration)?;
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        let (user_data, id) = self.begin_owned()?;
        let held = Held {
            guard: None,
            registration: Some(registration.begin_use(span, KernelAccess::WritesBuffer)),
        };
        // SAFETY: as `read_registered_raw`.
        let hr = unsafe {
            crate::sys::build_read(
                self.ring.raw_handle(),
                target,
                registered_buffer_ref(index, span.offset),
                span.len,
                file_offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_owned(hr, id, None, extra, held)
    }

    /// Queue a read into a registered buffer against a guarded file, with the
    /// **ring** holding both the use and the guard (`D-73`).
    ///
    /// The guarded counterpart to [`Batch::read_registered_raw_owned`], and the
    /// only shape that fills both halves of `Held`.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_registered_raw_owned`].
    pub fn read_registered_owned<B: IoBufMut, F: FileTarget>(
        &mut self,
        file: &F,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        extra: X,
        file_offset: u64,
        options: PushOptions,
    ) -> io::Result<OperationId> {
        self.require(Op::Read)?;
        self.check_registration_ring(registration)?;
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        let (user_data, id) = self.begin_owned()?;
        let held = Held {
            guard: Some(file.guard().into()),
            registration: Some(registration.begin_use(span, KernelAccess::WritesBuffer)),
        };
        // SAFETY: as `read_registered`.
        let hr = unsafe {
            crate::sys::build_read(
                self.ring.raw_handle(),
                target,
                registered_buffer_ref(index, span.offset),
                span.len,
                file_offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_owned(hr, id, None, extra, held)
    }

    /// Queue a write of `span.len` bytes to `file` at `file_offset`, from
    /// `span`'s byte offset of `registration`'s buffer at `span.buffer_index`
    /// (M5.2).
    ///
    /// As [`Batch::read_registered_raw_owned`], but for `BuildIoRingWriteFile`.
    /// Prefer [`Batch::write_registered_owned`] unless `file` needs to address a
    /// raw `FileRef` directly.
    ///
    /// # Safety
    ///
    /// As [`Batch::read_registered_raw_owned`]'s.
    ///
    /// # Errors
    ///
    /// As [`Batch::read_registered_raw_owned`].
    ///
    /// # Why the long parameter list
    ///
    /// These are `BuildIoRingWriteFile`'s own parameters, plus the sidecar the
    /// inventory carries. Collapsing them into a struct would hide which are
    /// the kernel's and which are this crate's, which is the distinction a
    /// reader of a push most needs. `sys.rs` takes the same view for the same
    /// reason.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the Win32 call plus the inventory sidecar"
    )]
    pub unsafe fn write_registered_raw_owned<B: IoBufMut>(
        &mut self,
        file: impl Into<FileRef>,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        extra: X,
        file_offset: u64,
        options: PushOptions,
        caching: WriteCaching,
    ) -> io::Result<OperationId> {
        self.require(Op::Write)?;
        self.check_registration_ring(registration)?;
        let target = handle_ref(file.into(), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        let (user_data, id) = self.begin_owned()?;
        let held = Held {
            guard: None,
            registration: Some(registration.begin_use(span, KernelAccess::ReadsBuffer)),
        };
        // SAFETY: as `write_registered_raw`.
        let hr = unsafe {
            crate::sys::build_write(
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
        self.finish_owned(hr, id, None, extra, held)
    }

    /// Queue a write from a registered buffer against a guarded file, with the
    /// **ring** holding both the use and the guard (`D-73`).
    ///
    /// The guarded counterpart to [`Batch::write_registered_raw_owned`].
    ///
    /// # Errors
    ///
    /// As [`Batch::write_registered_raw_owned`].
    ///
    /// # Why the long parameter list
    ///
    /// These are `BuildIoRingWriteFile`'s own parameters, plus the sidecar the
    /// inventory carries. Collapsing them into a struct would hide which are
    /// the kernel's and which are this crate's, which is the distinction a
    /// reader of a push most needs. `sys.rs` takes the same view for the same
    /// reason.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the Win32 call plus the inventory sidecar"
    )]
    pub fn write_registered_owned<B: IoBufMut, F: FileTarget>(
        &mut self,
        file: &F,
        registration: &RegisteredBuffers<B>,
        span: RegisteredSpan,
        extra: X,
        file_offset: u64,
        options: PushOptions,
        caching: WriteCaching,
    ) -> io::Result<OperationId> {
        self.require(Op::Write)?;
        self.check_registration_ring(registration)?;
        let target = handle_ref(file.as_file_ref(), self.ring.ring_id())?;
        let index = registration.checked_span(span)?;
        let (user_data, id) = self.begin_owned()?;
        let held = Held {
            guard: Some(file.guard().into()),
            registration: Some(registration.begin_use(span, KernelAccess::ReadsBuffer)),
        };
        // SAFETY: as `write_registered`.
        let hr = unsafe {
            crate::sys::build_write(
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
        self.finish_owned(hr, id, None, extra, held)
    }
}
