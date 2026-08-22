// Copyright (c) 2026 Mike Grier
//! Safe file-family operation adapters, gated behind the `fs` feature.
//!
//! These wrappers own the I/O buffer and issue the single native `ReadFile` /
//! `WriteFile` internally, so a caller performs file overlapped I/O without
//! touching `OVERLAPPED`, the submission seam, or `unsafe`. They are the file
//! family's realization of the per-family safe-adapter decision; other families
//! follow the same shape.

use std::alloc::{self, Layout};
use std::fmt;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::ptr::NonNull;
use std::slice;

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, FALSE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SEGMENT_ELEMENT, ReadFile, ReadFileScatter, WriteFile, WriteFileGather,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

use crate::operation::{payload_ptr_from_overlapped, sync_bytes_ptr_from_overlapped};
use crate::{
    AssociatedEndpoint, BlockingEndpoint, Completion, IoBuf, IoBufMut, Issued, Operation,
    OperationId, Started, Submitted,
};

impl BlockingEndpoint {
    /// Read into `buffer` starting at `offset`, blocking until the read
    /// completes, and return the number of bytes read.
    ///
    /// Takes a plain `&mut [u8]` rather than an owned buffer, and allocates
    /// nothing: this call does not return until the operation is over, so an
    /// ordinary borrow provably covers the whole time the kernel is writing.
    /// That is the difference from [`AssociatedEndpoint::read`], which must take
    /// ownership because its operation outlives the call.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `buffer` is longer than
    /// `u32::MAX`, which the read's byte count cannot express, or any error from
    /// issuing or completing the read.
    ///
    /// # Examples
    ///
    /// One owner issuing reads in sequence is the supported shape, and compiles:
    ///
    /// ```
    /// use windows_overlapped_io_sys::BlockingEndpoint;
    ///
    /// fn read_twice(endpoint: &mut BlockingEndpoint) -> std::io::Result<()> {
    ///     let mut buffer = [0_u8; 64];
    ///     let _first = endpoint.read(&mut buffer, 0)?;
    ///     let _second = endpoint.read(&mut buffer, 64)?;
    ///     Ok(())
    /// }
    /// ```
    ///
    /// Sharing one endpoint across threads and reading from both is rejected at
    /// compile time rather than corrupting a result at run time, because `read`
    /// takes `&mut self` while an `Arc` can only hand out `&BlockingEndpoint`:
    ///
    /// ```compile_fail
    /// use std::sync::Arc;
    /// use windows_overlapped_io_sys::BlockingEndpoint;
    ///
    /// fn read_from_two_threads(endpoint: BlockingEndpoint) {
    ///     let shared = Arc::new(endpoint);
    ///     let other = Arc::clone(&shared);
    ///     std::thread::spawn(move || other.read(&mut [0_u8; 64], 0));
    ///     let _ = shared.read(&mut [0_u8; 64], 64);
    /// }
    /// ```
    pub fn read(&mut self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        let buf_len = checked_len(buffer.len(), "read buffer")?;
        let buf_ptr = buffer.as_mut_ptr();

        let mut operation = Operation::new(());
        operation.set_offset(offset);
        // SAFETY: issues exactly one overlapped ReadFile into `buffer`, which
        // outlives this blocking call; no other operation is outstanding.
        unsafe {
            self.run(&mut operation, |handle, overlapped| {
                let ok = ReadFile(
                    handle.as_raw_handle(),
                    buf_ptr,
                    buf_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify(ok)
            })
        }
    }

    /// Write `data` starting at `offset`, blocking until the write completes, and
    /// return the number of bytes written.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `data` is longer than
    /// `u32::MAX`, which the write's byte count cannot express, or any error
    /// from issuing or completing the write.
    pub fn write(&mut self, data: &[u8], offset: u64) -> io::Result<usize> {
        let data_ptr = data.as_ptr();
        let data_len = checked_len(data.len(), "write buffer")?;

        let mut operation = Operation::new(());
        operation.set_offset(offset);
        // SAFETY: issues exactly one overlapped WriteFile from `data`, which
        // outlives this blocking call; no other operation is outstanding.
        let written = unsafe {
            self.run(&mut operation, |handle, overlapped| {
                let ok = WriteFile(
                    handle.as_raw_handle(),
                    data_ptr,
                    data_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify(ok)
            })
        }?;

        Ok(written)
    }
}

/// Map a native `BOOL` into the submission-seam contract: native success or
/// `ERROR_IO_PENDING` is accepted, any other error is an immediate failure.
fn classify(ok: i32) -> io::Result<()> {
    if ok != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        Ok(())
    } else {
        Err(error)
    }
}

/// Convert a buffer length to the `u32` byte count the Win32 calls take.
///
/// Rejects rather than caps, for the same reason as the device-control helper:
/// capping would transfer a prefix of the caller's buffer and then report
/// success for an operation that did something other than what was asked.
fn checked_len(len: usize, which: &str) -> io::Result<u32> {
    u32::try_from(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a {which} is limited to u32::MAX bytes; {len} does not fit"),
        )
    })
}

impl AssociatedEndpoint<'_> {
    /// Submit an overlapped read into `buffer`, starting at `offset`.
    ///
    /// The buffer is any owned [`IoBufMut`] -- a `Vec<u8>`, a `Box<[u8]>`, a
    /// [`PageBuffers`], or a caller's own pooled or aligned type -- handed over
    /// for the operation's life and returned when it completes. Nothing is
    /// copied and nothing is allocated here: a caller that wants a fresh `Vec`
    /// writes `vec![0; n]` at the call site, where the allocation is visible.
    ///
    /// Returns [`Started::Pending`] with a [`FileIo`] token that recovers the
    /// buffer and byte count from the operation's completion, or -- only on an
    /// endpoint in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode, where a
    /// synchronous success queues no packet -- [`Started::Completed`] with the
    /// buffer already in hand.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the buffer is longer than
    /// `u32::MAX`, or any immediate failure from issuing the read.
    #[track_caller]
    pub fn read<B: IoBufMut>(&self, buffer: B, offset: u64) -> io::Result<Started<FileIo<B>, B>> {
        let buf_len = checked_len(buffer.bytes_len(), "read buffer")?;
        let skip = self.notification_modes().skip_completion_port_on_success;
        let mut operation = Operation::new(buffer);
        operation.set_offset(offset);
        // SAFETY: issues exactly one ReadFile into the operation's own payload
        // buffer, reached through the pinned OVERLAPPED; `IoBufMut` promises that
        // address is stable and exclusively owned, and the payload and byte-count
        // cell live until the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<B>(overlapped);
                let bytes = sync_bytes_ptr_from_overlapped(overlapped);
                let ok = ReadFile(
                    handle.as_raw_handle(),
                    (*payload).stable_mut_ptr(),
                    buf_len,
                    bytes,
                    overlapped,
                );
                classify_issued(ok, skip, bytes)
            })
        };
        finish(submitted)
    }

    /// Submit an overlapped write of `buffer`, starting at `offset`.
    ///
    /// The buffer is any owned [`IoBuf`] -- including a shared `Arc<[u8]>` or a
    /// `&'static [u8]`, neither of which can be a read destination -- handed over
    /// for the operation's life and returned when it completes. Nothing is
    /// copied.
    ///
    /// Returns [`Started::Pending`] with a [`FileIo`] token, or
    /// [`Started::Completed`] with the buffer already in hand when the endpoint
    /// is in skip-on-success mode and the write completed synchronously.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the buffer is longer than
    /// `u32::MAX`, or any immediate failure from issuing the write.
    #[track_caller]
    pub fn write<B: IoBuf>(&self, buffer: B, offset: u64) -> io::Result<Started<FileIo<B>, B>> {
        let data_len = checked_len(buffer.bytes_len(), "write buffer")?;
        let skip = self.notification_modes().skip_completion_port_on_success;
        let mut operation = Operation::new(buffer);
        operation.set_offset(offset);
        // SAFETY: issues exactly one WriteFile from the operation's own payload
        // buffer, reached through the pinned OVERLAPPED; `IoBuf` promises that
        // address is stable and its bytes unmodified, and the payload and
        // byte-count cell live until the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<B>(overlapped);
                let bytes = sync_bytes_ptr_from_overlapped(overlapped);
                let ok = WriteFile(
                    handle.as_raw_handle(),
                    (*payload).stable_ptr(),
                    data_len,
                    bytes,
                    overlapped,
                );
                classify_issued(ok, skip, bytes)
            })
        };
        finish(submitted)
    }
}

/// Map a native `BOOL` into the IOCP submission contract.
///
/// # Why an immediate `TRUE` is usually `Pending`
///
/// [`Issued`] does not record whether the call finished synchronously. It
/// records whether a **completion packet will arrive**, and for an IOCP-bound
/// overlapped handle those are different facts: the I/O Manager queues a packet
/// for every request it completes, *including* one that succeeded immediately
/// without returning `ERROR_IO_PENDING`. See [`Issued::Pending`].
///
/// The single exception is `skip_on_success`, which is why this needs to know
/// it: on an endpoint in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode no packet
/// is queued for an immediate success, so that -- and only that -- is an
/// [`Issued::Completed`]. Getting this backwards in either direction is a bug
/// with teeth: claiming `Completed` when a packet is coming frees the operation
/// under a live `OVERLAPPED`, and claiming `Pending` when none is coming leaves
/// the operation outstanding forever and wedges rundown.
///
/// # Safety
///
/// `sync_bytes` must be the byte-count cell of the operation being submitted,
/// which is live for the whole call.
unsafe fn classify_issued(
    ok: i32,
    skip_on_success: bool,
    sync_bytes: *mut u32,
) -> io::Result<Issued> {
    if ok != 0 {
        if skip_on_success {
            // SAFETY: the call reported immediate success, so the kernel has
            // already written the count and will not write it again.
            let bytes_transferred = unsafe { *sync_bytes };
            return Ok(Issued::Completed { bytes_transferred });
        }
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        Ok(Issued::Pending)
    } else {
        Err(error)
    }
}

/// As [`classify_issued`], for the scatter/gather calls.
///
/// `ReadFileScatter` and `WriteFileGather` take no byte-count out-parameter --
/// the slot in that position is `lpReserved` and must be null -- so on the
/// skip-on-success path the count comes from `GetOverlappedResult` instead.
/// That is the sanctioned way to read it (`Internal`/`InternalHigh` are never
/// touched directly), and it cannot block here: it is called only after the
/// call reported immediate success, so the operation is already complete and
/// `bWait` is `FALSE`.
///
/// # Safety
///
/// `handle` must be the endpoint's live handle and `overlapped` the identity of
/// the operation just submitted through it.
unsafe fn classify_scatter(
    ok: i32,
    skip_on_success: bool,
    handle: std::os::windows::io::RawHandle,
    overlapped: *mut OVERLAPPED,
) -> io::Result<Issued> {
    if ok != 0 {
        if skip_on_success {
            let mut bytes_transferred = 0_u32;
            // SAFETY: a live handle and the completed operation's own
            // OVERLAPPED; `bWait` is FALSE, so this only reads what is already
            // recorded.
            let got =
                unsafe { GetOverlappedResult(handle, overlapped, &mut bytes_transferred, FALSE) };
            if got == 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(Issued::Completed { bytes_transferred });
        }
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        Ok(Issued::Pending)
    } else {
        Err(error)
    }
}

/// Turn a submission outcome into the adapter's two-state outcome.
fn finish<B: IoBuf>(submitted: Submitted<B>) -> io::Result<Started<FileIo<B>, B>> {
    match submitted {
        Submitted::Pending(id) => Ok(Started::Pending(FileIo {
            id,
            buffer: std::marker::PhantomData,
        })),
        Submitted::Completed {
            operation,
            bytes_transferred,
        } => Ok(Started::Completed {
            payload: operation.into_payload(),
            bytes_transferred: bytes_transferred as usize,
        }),
        Submitted::Failed { error, .. } => Err(error),
    }
}

/// A pending file operation submitted through [`AssociatedEndpoint::read`] or
/// [`AssociatedEndpoint::write`].
///
/// The token carries the operation's identity and remembers the buffer type it
/// was submitted with, so [`FileIo::claim`] hands back the caller's own buffer
/// -- the same value, not a copy -- once the matching completion is dequeued.
#[derive(Debug)]
pub struct FileIo<B> {
    id: OperationId,
    /// The buffer itself is in the pinned operation, not here; this only keeps
    /// the token's type tied to it so `claim` cannot be handed the wrong one.
    buffer: std::marker::PhantomData<fn() -> B>,
}

impl<B: IoBuf> FileIo<B> {
    /// The identity of the in-flight operation, for cancellation or matching.
    #[must_use]
    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Claim this operation's result from `completion`.
    ///
    /// On a match returns `Ok((buffer, result))`: `buffer` is the one the caller
    /// handed over -- the bytes read, or the data written -- and `result` is the
    /// byte count or the operation's error. Returns `Err(self)` when
    /// `completion` belongs to a different operation, so the caller can try the
    /// token against another one.
    pub fn claim(self, completion: &Completion) -> Result<(B, io::Result<usize>), Self> {
        if completion.id() != Some(self.id) {
            return Err(self);
        }
        // SAFETY: the full identity -- address *and* generation -- matches, which
        // an address alone would not: a recycled address can belong to a later
        // operation of a different payload type. The match therefore proves this
        // completion is the Operation<B> this token submitted, and the token's
        // own type parameter names that B; claim it exactly once.
        let operation = unsafe { completion.claim::<B>() };
        let buffer = operation.into_payload();
        let result = match completion.error() {
            Some(error) => Err(io::Error::from_raw_os_error(
                error.raw_os_error().unwrap_or_default(),
            )),
            None => Ok(completion.bytes_transferred() as usize),
        };
        Ok((buffer, result))
    }
}

/// The memory page size assumed by the scatter/gather adapters.
///
/// A fixed 4 KiB, matching every Windows target this crate supports. Buffers are
/// aligned to it and I/O lengths are multiples of it, which also satisfies the
/// sector alignment `FILE_FLAG_NO_BUFFERING` requires.
pub const PAGE_SIZE: usize = 4096;

/// The Win32 `FILE_FLAG_NO_BUFFERING` flag.
///
/// The scatter/gather adapters require the endpoint be opened with this flag (in
/// addition to `FILE_FLAG_OVERLAPPED`, which [`crate::UnassociatedEndpoint::open`]
/// always sets); pass it as that constructor's `extra_flags`.
pub const FILE_FLAG_NO_BUFFERING: u32 =
    windows_sys::Win32::Storage::FileSystem::FILE_FLAG_NO_BUFFERING;

/// A page-aligned set of memory pages: the buffer form the scatter/gather
/// adapters read into and write from.
///
/// It owns one page-aligned allocation of `pages * PAGE_SIZE` bytes and can be
/// viewed as a byte slice. Its page-aligned segments are what `ReadFileScatter`
/// and `WriteFileGather` require.
pub struct PageBuffers {
    ptr: NonNull<u8>,
    pages: usize,
}

// SAFETY: `PageBuffers` uniquely owns its heap allocation; moving it between
// threads moves that ownership, and it hands out aliasing access only through
// `&`/`&mut self`, so it is as `Send`/`Sync` as an owned `Box<[u8]>`.
unsafe impl Send for PageBuffers {}
unsafe impl Sync for PageBuffers {}

impl PageBuffers {
    /// Allocate `pages` zeroed, page-aligned memory pages.
    ///
    /// # Panics
    ///
    /// Panics if `pages` is zero or `pages * PAGE_SIZE` overflows.
    #[must_use]
    pub fn new(pages: usize) -> Self {
        assert!(pages > 0, "PageBuffers requires at least one page");
        let size = pages
            .checked_mul(PAGE_SIZE)
            .expect("page buffer size overflow");
        let layout = Layout::from_size_align(size, PAGE_SIZE).expect("valid page layout");
        // SAFETY: `layout` has non-zero size.
        let raw = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self { ptr, pages }
    }

    /// The number of pages.
    #[must_use]
    pub fn pages(&self) -> usize {
        self.pages
    }

    /// The total length in bytes (`pages * PAGE_SIZE`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.pages * PAGE_SIZE
    }

    /// Always `false`: a `PageBuffers` holds at least one page.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// View the pages as a shared byte slice.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: `ptr` owns `len()` initialized bytes for the shared borrow.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len()) }
    }

    /// View the pages as a mutable byte slice.
    #[must_use]
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: exclusive borrow of `len()` bytes this owns.
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len()) }
    }

    /// Build the `NULL`-terminated `FILE_SEGMENT_ELEMENT` array over these pages.
    fn segment_array(&self) -> Vec<FILE_SEGMENT_ELEMENT> {
        let mut segments = Vec::with_capacity(self.pages + 1);
        for i in 0..self.pages {
            // SAFETY: `i < pages`, so the offset stays within the allocation, and
            // each page start is page-aligned because the base is.
            let page = unsafe { self.ptr.as_ptr().add(i * PAGE_SIZE) };
            segments.push(FILE_SEGMENT_ELEMENT {
                Buffer: page.cast(),
            });
        }
        // A zeroed element terminates the array.
        segments.push(FILE_SEGMENT_ELEMENT { Alignment: 0 });
        segments
    }
}

impl fmt::Debug for PageBuffers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageBuffers")
            .field("pages", &self.pages)
            .finish_non_exhaustive()
    }
}

impl Drop for PageBuffers {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.len(), PAGE_SIZE).expect("valid page layout");
        // SAFETY: `ptr` came from `alloc_zeroed` with this exact layout.
        unsafe { alloc::dealloc(self.ptr.as_ptr(), layout) };
    }
}

// SAFETY: the bytes live in a page-aligned heap allocation `PageBuffers` owns
// outright, so moving the value moves the pointer and not the bytes, and the
// page count is fixed at construction. `alloc_zeroed` initializes all of them.
unsafe impl crate::IoBuf for PageBuffers {
    fn stable_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.len()
    }
}

// SAFETY: as above; `PageBuffers` is a unique owner, so `&mut self` is exclusive
// access to the same allocation `stable_ptr` reports.
unsafe impl crate::IoBufMut for PageBuffers {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }
}

impl BlockingEndpoint {
    /// Scatter-read into `buffers` starting at `offset`, blocking until the read
    /// completes, and return the number of bytes read.
    ///
    /// Takes the caller's pages by `&mut` and allocates nothing, matching
    /// [`BlockingEndpoint::write_gather`]; the endpoint must be opened with
    /// [`FILE_FLAG_NO_BUFFERING`], or the native call fails.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the pages total more than
    /// `u32::MAX` bytes, or any error from issuing or completing the
    /// scatter-read.
    pub fn read_scatter(&mut self, buffers: &mut PageBuffers, offset: u64) -> io::Result<usize> {
        let total = checked_len(buffers.len(), "scatter/gather buffer set")?;
        let segments = buffers.segment_array();
        let seg_ptr = segments.as_ptr();

        let mut operation = Operation::new(());
        operation.set_offset(offset);
        // SAFETY: issues exactly one ReadFileScatter into `buffers` via
        // `segments`; both outlive this blocking call and no other operation is
        // outstanding.
        unsafe {
            self.run(&mut operation, |handle, overlapped| {
                let ok = ReadFileScatter(
                    handle.as_raw_handle(),
                    seg_ptr,
                    total,
                    std::ptr::null(),
                    overlapped,
                );
                classify(ok)
            })
        }
    }

    /// Gather-write `buffers` starting at `offset`, blocking until the write
    /// completes, and return the number of bytes written.
    ///
    /// The endpoint must be opened with [`FILE_FLAG_NO_BUFFERING`]; otherwise the
    /// native call fails.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the buffers total more than
    /// `u32::MAX` bytes, or any error from issuing or completing the
    /// gather-write.
    pub fn write_gather(&mut self, buffers: &PageBuffers, offset: u64) -> io::Result<usize> {
        let segments = buffers.segment_array();
        let total = checked_len(buffers.len(), "scatter/gather buffer set")?;
        let seg_ptr = segments.as_ptr();

        let mut operation = Operation::new(());
        operation.set_offset(offset);
        // SAFETY: issues exactly one WriteFileGather from `buffers` via
        // `segments`; both outlive this blocking call and no other operation is
        // outstanding.
        let written = unsafe {
            self.run(&mut operation, |handle, overlapped| {
                let ok = WriteFileGather(
                    handle.as_raw_handle(),
                    seg_ptr,
                    total,
                    std::ptr::null(),
                    overlapped,
                );
                classify(ok)
            })
        }?;

        Ok(written)
    }
}

/// The pinned payload for an in-flight scatter/gather operation: the buffers and
/// the `FILE_SEGMENT_ELEMENT` array that points into them.
struct ScatterPayload {
    buffers: PageBuffers,
    segments: Vec<FILE_SEGMENT_ELEMENT>,
}

// SAFETY: the raw pointers in `segments` point into `buffers`, which this payload
// owns; moving the payload moves the whole self-referential unit together, and it
// exposes no aliasing access, so it is `Send` like the `PageBuffers` it wraps.
unsafe impl Send for ScatterPayload {}

impl AssociatedEndpoint<'_> {
    /// Submit an overlapped scatter-read into `buffers`, starting at `offset`.
    ///
    /// Takes the caller's pages rather than allocating fresh ones, so a pooled
    /// or reused [`PageBuffers`] costs nothing to submit. The endpoint must be
    /// opened with [`FILE_FLAG_NO_BUFFERING`].
    ///
    /// Returns [`Started::Pending`] with a [`ScatterGatherIo`] token, or
    /// [`Started::Completed`] with the [`PageBuffers`] already in hand when the
    /// endpoint is in skip-on-success mode and the read completed synchronously.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the pages total more than
    /// `u32::MAX` bytes, or any immediate failure from issuing the
    /// scatter-read.
    #[track_caller]
    pub fn read_scatter(
        &self,
        buffers: PageBuffers,
        offset: u64,
    ) -> io::Result<Started<ScatterGatherIo, PageBuffers>> {
        let total = checked_len(buffers.len(), "scatter/gather buffer set")?;
        let skip = self.notification_modes().skip_completion_port_on_success;
        let segments = buffers.segment_array();
        let mut operation = Operation::new(ScatterPayload { buffers, segments });
        operation.set_offset(offset);
        // SAFETY: issues exactly one ReadFileScatter into the payload's buffers
        // via its segment array, both reached through the pinned OVERLAPPED; they
        // live until the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<ScatterPayload>(overlapped);
                let raw = handle.as_raw_handle();
                let ok = ReadFileScatter(
                    raw,
                    (*payload).segments.as_ptr(),
                    total,
                    std::ptr::null(),
                    overlapped,
                );
                classify_scatter(ok, skip, raw, overlapped)
            })
        };
        finish_scatter(submitted)
    }

    /// Submit an overlapped gather-write of `buffers` starting at `offset`.
    ///
    /// Returns [`Started::Pending`] with a [`ScatterGatherIo`] token, or
    /// [`Started::Completed`] with the [`PageBuffers`] already in hand when the
    /// endpoint is in skip-on-success mode and the write completed
    /// synchronously. The endpoint must be opened with
    /// [`FILE_FLAG_NO_BUFFERING`].
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the buffers total more than
    /// `u32::MAX` bytes, or any immediate failure from issuing the gather-write.
    #[track_caller]
    pub fn write_gather(
        &self,
        buffers: PageBuffers,
        offset: u64,
    ) -> io::Result<Started<ScatterGatherIo, PageBuffers>> {
        let total = checked_len(buffers.len(), "scatter/gather buffer set")?;
        let skip = self.notification_modes().skip_completion_port_on_success;
        let segments = buffers.segment_array();
        let mut operation = Operation::new(ScatterPayload { buffers, segments });
        operation.set_offset(offset);
        // SAFETY: issues exactly one WriteFileGather from the payload's buffers
        // via its segment array, both reached through the pinned OVERLAPPED; they
        // live until the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<ScatterPayload>(overlapped);
                let raw = handle.as_raw_handle();
                let ok = WriteFileGather(
                    raw,
                    (*payload).segments.as_ptr(),
                    total,
                    std::ptr::null(),
                    overlapped,
                );
                classify_scatter(ok, skip, raw, overlapped)
            })
        };
        finish_scatter(submitted)
    }
}

/// Turn a scatter/gather submission outcome into the adapter's two-state
/// outcome.
fn finish_scatter(
    submitted: Submitted<ScatterPayload>,
) -> io::Result<Started<ScatterGatherIo, PageBuffers>> {
    match submitted {
        Submitted::Pending(id) => Ok(Started::Pending(ScatterGatherIo { id })),
        Submitted::Completed {
            operation,
            bytes_transferred,
        } => Ok(Started::Completed {
            payload: operation.into_payload().buffers,
            bytes_transferred: bytes_transferred as usize,
        }),
        Submitted::Failed { error, .. } => Err(error),
    }
}

/// A pending scatter/gather operation submitted through
/// [`AssociatedEndpoint::read_scatter`] or [`AssociatedEndpoint::write_gather`].
///
/// The token carries the operation's identity and its payload type, so
/// [`ScatterGatherIo::claim`] recovers the [`PageBuffers`] and byte count safely
/// once the matching completion is dequeued.
#[derive(Debug)]
pub struct ScatterGatherIo {
    id: OperationId,
}

impl ScatterGatherIo {
    /// The identity of the in-flight operation, for cancellation or matching.
    #[must_use]
    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Claim this operation's result from `completion`.
    ///
    /// On a match returns `Ok((buffers, result))`: `buffers` is the payload (the
    /// pages read, or the data written) and `result` is the byte count or the
    /// operation's error. Returns `Err(self)` when `completion` belongs to a
    /// different operation.
    pub fn claim(self, completion: &Completion) -> Result<(PageBuffers, io::Result<usize>), Self> {
        if completion.id() != Some(self.id) {
            return Err(self);
        }
        // SAFETY: the full identity -- address *and* generation -- matches, which
        // an address alone would not: a recycled address can belong to a later
        // operation of a different payload type. The match therefore proves this
        // completion is the
        // Operation<ScatterPayload> this token submitted; claim it exactly once.
        let operation = unsafe { completion.claim::<ScatterPayload>() };
        let buffers = operation.into_payload().buffers;
        let result = match completion.error() {
            Some(error) => Err(io::Error::from_raw_os_error(
                error.raw_os_error().unwrap_or_default(),
            )),
            None => Ok(completion.bytes_transferred() as usize),
        };
        Ok((buffers, result))
    }
}

#[cfg(test)]
mod tests;
