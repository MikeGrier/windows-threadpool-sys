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

use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SEGMENT_ELEMENT, ReadFile, ReadFileScatter, WriteFile, WriteFileGather,
};

use crate::operation::payload_ptr_from_overlapped;
use crate::{
    AssociatedEndpoint, BlockingEndpoint, Completion, Issued, Operation, OperationId, Submitted,
};

impl BlockingEndpoint {
    /// Read up to `len` bytes starting at `offset`, blocking until the read
    /// completes.
    ///
    /// Returns the buffer truncated to the bytes actually read, together with
    /// that count. The whole operation finishes within this call, so no
    /// `OVERLAPPED` or `unsafe` reaches the caller.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the read.
    pub fn read(&self, len: usize, offset: u64) -> io::Result<(Vec<u8>, usize)> {
        let mut buffer = vec![0_u8; len];
        let buf_ptr = buffer.as_mut_ptr();
        let buf_len = clamp_u32(len);

        let mut operation = Operation::new(());
        operation.set_offset(offset);
        // SAFETY: issues exactly one overlapped ReadFile into `buffer`, which
        // outlives this blocking call; no other operation is outstanding.
        let read = unsafe {
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
        }?;

        buffer.truncate(read);
        Ok((buffer, read))
    }

    /// Write `data` starting at `offset`, blocking until the write completes, and
    /// return the number of bytes written.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the write.
    pub fn write(&self, data: &[u8], offset: u64) -> io::Result<usize> {
        let data_ptr = data.as_ptr();
        let data_len = clamp_u32(data.len());

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

/// Clamp a length to the `u32` byte-count parameter the Win32 calls take.
fn clamp_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

impl AssociatedEndpoint<'_> {
    /// Submit an overlapped read of up to `len` bytes starting at `offset`,
    /// returning a [`FileIo`] token that recovers the buffer and byte count from
    /// the operation's completion.
    ///
    /// The endpoint must not be in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode;
    /// this adapter always expects a completion packet to arrive.
    ///
    /// # Errors
    ///
    /// Returns any immediate failure from issuing the read.
    #[track_caller]
    pub fn read(&self, len: usize, offset: u64) -> io::Result<FileIo> {
        let buf_len = clamp_u32(len);
        let mut operation = Operation::new(vec![0_u8; len]);
        operation.set_offset(offset);
        // SAFETY: issues exactly one ReadFile into the operation's own payload
        // buffer, reached through the pinned OVERLAPPED; the payload lives until
        // the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<Vec<u8>>(overlapped);
                let ok = ReadFile(
                    handle.as_raw_handle(),
                    (*payload).as_mut_ptr(),
                    buf_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify_issued(ok)
            })
        };
        finish(submitted)
    }

    /// Submit an overlapped write of `data` starting at `offset`, returning a
    /// [`FileIo`] token that recovers the buffer and byte count from the
    /// operation's completion.
    ///
    /// The endpoint must not be in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode.
    ///
    /// # Errors
    ///
    /// Returns any immediate failure from issuing the write.
    #[track_caller]
    pub fn write(&self, data: Vec<u8>, offset: u64) -> io::Result<FileIo> {
        let data_len = clamp_u32(data.len());
        let mut operation = Operation::new(data);
        operation.set_offset(offset);
        // SAFETY: issues exactly one WriteFile from the operation's own payload
        // buffer, reached through the pinned OVERLAPPED; the payload lives until
        // the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<Vec<u8>>(overlapped);
                let ok = WriteFile(
                    handle.as_raw_handle(),
                    (*payload).as_ptr(),
                    data_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify_issued(ok)
            })
        };
        finish(submitted)
    }
}

/// Map a native `BOOL` into the IOCP submission contract, expecting a completion
/// packet on success because the adapter never enables skip-on-success mode.
fn classify_issued(ok: i32) -> io::Result<Issued> {
    if ok != 0 {
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        Ok(Issued::Pending)
    } else {
        Err(error)
    }
}

/// Turn a submission outcome into a [`FileIo`] token or an immediate error.
fn finish(submitted: Submitted<Vec<u8>>) -> io::Result<FileIo> {
    match submitted {
        Submitted::Pending(id) => Ok(FileIo { id }),
        Submitted::Completed { .. } => Err(io::Error::other(
            "file adapter observed a synchronous completion; the endpoint must not be in \
             FILE_SKIP_COMPLETION_PORT_ON_SUCCESS mode",
        )),
        Submitted::Failed { error, .. } => Err(error),
    }
}

/// A pending file operation submitted through [`AssociatedEndpoint::read`] or
/// [`AssociatedEndpoint::write`].
///
/// The token carries the operation's identity and its `Vec<u8>` payload type, so
/// [`FileIo::claim`] recovers the buffer and byte count safely once the matching
/// completion is dequeued.
#[derive(Debug)]
pub struct FileIo {
    id: OperationId,
}

impl FileIo {
    /// The identity of the in-flight operation, for cancellation or matching.
    #[must_use]
    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Claim this operation's result from `completion`.
    ///
    /// On a match returns `Ok((buffer, result))`: `buffer` is the payload -- the
    /// bytes read, or the data written -- and `result` is the byte count or the
    /// operation's error. Returns `Err(self)` when `completion` belongs to a
    /// different operation, so the caller can try the token against another one.
    pub fn claim(self, completion: &Completion) -> Result<(Vec<u8>, io::Result<usize>), Self> {
        if completion.overlapped_ptr() != self.id.as_ptr() {
            return Err(self);
        }
        // SAFETY: the identity match proves this completion is the
        // Operation<Vec<u8>> this token submitted; claim it exactly once.
        let operation = unsafe { completion.claim::<Vec<u8>>() };
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

impl BlockingEndpoint {
    /// Scatter-read `pages` pages starting at `offset` into a fresh page-aligned
    /// buffer, blocking until the read completes.
    ///
    /// Returns the buffer and the number of bytes read. The endpoint must be
    /// opened with [`FILE_FLAG_NO_BUFFERING`]; otherwise the native call fails.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the scatter-read.
    pub fn read_scatter(&self, pages: usize, offset: u64) -> io::Result<(PageBuffers, usize)> {
        let buffers = PageBuffers::new(pages);
        let segments = buffers.segment_array();
        let total = clamp_u32(buffers.len());
        let seg_ptr = segments.as_ptr();

        let mut operation = Operation::new(());
        operation.set_offset(offset);
        // SAFETY: issues exactly one ReadFileScatter into `buffers` via
        // `segments`; both outlive this blocking call and no other operation is
        // outstanding.
        let read = unsafe {
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
        }?;

        Ok((buffers, read))
    }

    /// Gather-write `buffers` starting at `offset`, blocking until the write
    /// completes, and return the number of bytes written.
    ///
    /// The endpoint must be opened with [`FILE_FLAG_NO_BUFFERING`]; otherwise the
    /// native call fails.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the gather-write.
    pub fn write_gather(&self, buffers: &PageBuffers, offset: u64) -> io::Result<usize> {
        let segments = buffers.segment_array();
        let total = clamp_u32(buffers.len());
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
    /// Submit an overlapped scatter-read of `pages` pages starting at `offset`
    /// into a fresh page-aligned buffer, returning a [`ScatterGatherIo`] token.
    ///
    /// The endpoint must be opened with [`FILE_FLAG_NO_BUFFERING`] and must not be
    /// in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode.
    ///
    /// # Errors
    ///
    /// Returns any immediate failure from issuing the scatter-read.
    #[track_caller]
    pub fn read_scatter(&self, pages: usize, offset: u64) -> io::Result<ScatterGatherIo> {
        let buffers = PageBuffers::new(pages);
        let total = clamp_u32(buffers.len());
        let segments = buffers.segment_array();
        let mut operation = Operation::new(ScatterPayload { buffers, segments });
        operation.set_offset(offset);
        // SAFETY: issues exactly one ReadFileScatter into the payload's buffers
        // via its segment array, both reached through the pinned OVERLAPPED; they
        // live until the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<ScatterPayload>(overlapped);
                let ok = ReadFileScatter(
                    handle.as_raw_handle(),
                    (*payload).segments.as_ptr(),
                    total,
                    std::ptr::null(),
                    overlapped,
                );
                classify_issued(ok)
            })
        };
        finish_scatter(submitted)
    }

    /// Submit an overlapped gather-write of `buffers` starting at `offset`,
    /// returning a [`ScatterGatherIo`] token.
    ///
    /// The endpoint must be opened with [`FILE_FLAG_NO_BUFFERING`] and must not be
    /// in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode.
    ///
    /// # Errors
    ///
    /// Returns any immediate failure from issuing the gather-write.
    #[track_caller]
    pub fn write_gather(&self, buffers: PageBuffers, offset: u64) -> io::Result<ScatterGatherIo> {
        let total = clamp_u32(buffers.len());
        let segments = buffers.segment_array();
        let mut operation = Operation::new(ScatterPayload { buffers, segments });
        operation.set_offset(offset);
        // SAFETY: issues exactly one WriteFileGather from the payload's buffers
        // via its segment array, both reached through the pinned OVERLAPPED; they
        // live until the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<ScatterPayload>(overlapped);
                let ok = WriteFileGather(
                    handle.as_raw_handle(),
                    (*payload).segments.as_ptr(),
                    total,
                    std::ptr::null(),
                    overlapped,
                );
                classify_issued(ok)
            })
        };
        finish_scatter(submitted)
    }
}

/// Turn a scatter/gather submission outcome into a token or an immediate error.
fn finish_scatter(submitted: Submitted<ScatterPayload>) -> io::Result<ScatterGatherIo> {
    match submitted {
        Submitted::Pending(id) => Ok(ScatterGatherIo { id }),
        Submitted::Completed { .. } => Err(io::Error::other(
            "scatter/gather adapter observed a synchronous completion; the endpoint must not be in \
             FILE_SKIP_COMPLETION_PORT_ON_SUCCESS mode",
        )),
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
        if completion.overlapped_ptr() != self.id.as_ptr() {
            return Err(self);
        }
        // SAFETY: the identity match proves this completion is the
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
