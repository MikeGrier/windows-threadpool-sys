// Copyright (c) 2026 Mike Grier
//! The reading layer, as a caller outside the crate holds it (TW-9.4).

use std::sync::Arc;
use std::{fmt, io};

use wtf_string::Wtf16Str;

use super::options::TailReaderOptions;
use super::tail_reader_error::TailReaderError;
use crate::pool::BufferPool;
use crate::read_engine::{ReadEngine, Tracked};
use crate::read_layer::ReadLayer;
use crate::read_sq::ReadSq;
use crate::{DiagnosticSink, NoDiagnostics, OpenError, PollError, Registration, Watcher};

#[cfg(test)]
mod tests;

/// Watches files for growth and reads the bytes they gain, on the thread pool, on a beat the caller
/// owns.
///
/// Each registered file is opened once and its handle kept. [`poll`](Self::poll) starts a poll and
/// returns; what the poll finds is read through an I/O ring into buffers from a pool, and arrives
/// as [`ReadEntry`](super::ReadEntry) entries on [`read_sq`](Self::read_sq), which the caller
/// drains. Each data entry carries a clone of the value the file was registered with, the range of
/// the file its bytes are from, and a [`Chunk`](super::Chunk) of them. A poll's last entry is
/// [`ReadEntry::PollDone`](super::ReadEntry::PollDone). Nothing runs between polls: no thread, no
/// timer, no wake.
///
/// The sizes it works to are the caller's, in a [`TailReaderOptions`].
///
/// # Example
///
/// ```
/// use std::os::windows::io::{AsRawHandle, BorrowedHandle};
///
/// use win_multi_file_tail_watcher::reader::{ReadEntry, TailReader, TailReaderOptions};
/// use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
/// use windows_sys::Win32::System::Threading::WaitForSingleObject;
/// use wtf_string::Wtf16String;
///
/// # fn wait(event: BorrowedHandle<'_>) {
/// #     // SAFETY: the handle is a live event that the reader owns and the caller borrows.
/// #     let waited = unsafe { WaitForSingleObject(event.as_raw_handle(), 10_000) };
/// #     assert_eq!(waited, WAIT_OBJECT_0);
/// # }
/// # let path = std::env::temp_dir().join(format!("tail-reader-doc-{}.log", std::process::id()));
/// # std::fs::write(&path, b"first line\n")?;
/// // The sizes, and a capacity for the watcher's own queue; the options' defaults are 64 buffers
/// // of 64 bytes, a client queue of 64 and a ring of 64 and 128.
/// let reader = TailReader::<u32>::new(&TailReaderOptions::default(), 64)?;
///
/// // Register a file from the start, with a value that comes back on every entry about it.
/// let registration = reader.register(&Wtf16String::from(path.as_os_str()), 0, 7)?;
/// assert_eq!(registration.value(), 7);
///
/// // Start a poll, then take what it delivers until its poll-done.
/// reader.poll()?;
/// let mut bytes = Vec::new();
/// 'poll: loop {
///     wait(reader.read_sq().doorbell());
///     // Reset first and then drain, so an entry written after the reset rings it again.
///     reader.read_sq().reset_doorbell();
///     for entry in reader.read_sq().drain() {
///         match entry {
///             // This file is shorter than a buffer, so it is one chunk. The chunks of a longer
///             // one arrive in any order, and `range()` says where each belongs.
///             ReadEntry::Data(data) => bytes.extend_from_slice(data.chunk()),
///             ReadEntry::PollDone => break 'poll,
///             _ => {}
///         }
///     }
/// }
/// assert_eq!(bytes, b"first line\n");
/// # drop(registration);
/// # drop(reader);
/// # std::fs::remove_file(&path)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # What holding things up does
///
/// The reader stops without waiting, and goes on by itself, for three reasons: every buffer is held
/// in a [`Chunk`](super::Chunk) the caller has not dropped, the I/O ring is full, or the client queue
/// is full because the caller has not drained it. Nothing is lost while it is stopped. Keeping a
/// chunk keeps its buffer out of the pool, so copy what is needed out of one and drop it.
///
/// # Callbacks and what they may do
///
/// `T::clone`, `T`'s `Drop` and the [`DiagnosticSink`] run on thread-pool callbacks, and a panic
/// that unwinds out of a callback aborts the process. A `T` is dropped on a callback, so its `Drop`
/// must not release the last owner of the reader, which waits for that callback when it is dropped.
/// A `Clone` that calls back into the reader (registering, or dropping a [`TailRegistration`])
/// deadlocks.
///
/// # Dropping it
///
/// Dropping the reader stops it as [`shutdown`](Self::shutdown) does and then waits for the crate's
/// own callbacks: a poll that is running reaches its next step and returns, and the reads already in
/// flight complete. It never waits for the caller to drain, nor for a poll-done to be written or
/// read, so it may be dropped with entries still queued.
pub struct TailReader<T: Clone + Send + 'static> {
    layer: ReadLayer<T>,
}

impl<T: Clone + Send + 'static> TailReader<T> {
    /// A reader working to `options`, over a watcher whose own submission queue holds
    /// `watcher_sq_capacity` entries, which must be a power of two of at least two as the client
    /// queue's capacity must.
    ///
    /// The options are checked first and nothing is allocated for a size they refuse. Diagnostics
    /// are discarded: [`with_diagnostics`](Self::with_diagnostics) supplies a sink.
    ///
    /// # Errors
    ///
    /// A [`TailReaderError`] naming the cause: a refused size, a capacity the watcher cannot have,
    /// a queue or buffers the allocator would not give, an I/O ring that could not be made, or
    /// something else the operating system refused.
    pub fn new(
        options: &TailReaderOptions,
        watcher_sq_capacity: usize,
    ) -> Result<Self, TailReaderError> {
        Self::with_diagnostics(options, watcher_sq_capacity, Arc::new(NoDiagnostics))
    }

    /// As [`new`](Self::new), with `diagnostics` told what the reader can say about itself: a probe
    /// or a read that failed, as [`Watcher::with_diagnostics`] does.
    ///
    /// The sink is called from whatever thread observed the event, so it must return promptly and
    /// must not call back into the reader.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_diagnostics(
        options: &TailReaderOptions,
        watcher_sq_capacity: usize,
        diagnostics: Arc<dyn DiagnosticSink>,
    ) -> Result<Self, TailReaderError> {
        options.validate()?;
        // The watcher first: it is small, and the buffers are not.
        let watcher = Watcher::with_diagnostics(watcher_sq_capacity, diagnostics)
            .map_err(TailReaderError::Watcher)?;
        let pool = BufferPool::new(options.buffer_len, options.buffer_count)
            .map_err(TailReaderError::from_pool)?;
        let engine = ReadEngine::new(
            watcher,
            pool,
            options.client_sq_capacity,
            options.ring_sq_size,
            options.ring_cq_size,
        )
        .map_err(TailReaderError::from_engine)?;
        let layer = ReadLayer::new(engine).map_err(TailReaderError::Io)?;
        Ok(Self { layer })
    }

    /// Register the file at `path`, starting at `offset` bytes into it, with `value`, which comes
    /// back in a clone on every entry about the file.
    ///
    /// The file is opened now and held until the [`TailRegistration`] drops. An offset past the
    /// file's length is refused. There is no uniqueness and no lookup by name: registering one path
    /// twice gives two independent registrations. A file registered while a poll is running may or
    /// may not be in that poll, and is in the next.
    ///
    /// # Errors
    ///
    /// An [`OpenError`], carrying the Win32 code of a failed open, or naming an offset past the end.
    pub fn register(
        &self,
        path: &Wtf16Str,
        offset: u64,
        value: T,
    ) -> Result<TailRegistration<T>, OpenError> {
        self.layer
            .register(path, offset, value)
            .map(|inner| TailRegistration { inner })
    }

    /// Start a poll and return at once. What it finds is read, and arrives on
    /// [`read_sq`](Self::read_sq) with no further call, ending with a poll-done. One poll runs at a
    /// time: a call while one is running or parked on a full queue is refused.
    ///
    /// # Errors
    ///
    /// A [`PollError`]: a poll is already running, the reader has been shut down, or the thread pool
    /// refused to start the poll, in which case a later call tries again.
    pub fn poll(&self) -> Result<(), PollError> {
        self.layer.poll()
    }

    /// The queue the caller drains, with the two doorbells it waits and rings on.
    #[must_use]
    pub fn read_sq(&self) -> &ReadSq<T> {
        self.layer.read_sq()
    }

    /// Stop for good, without blocking, and as often as asked. No poll starts, a poll running stops
    /// at its next step, no read is planned or submitted, and reads already in flight complete and
    /// are delivered. Every poll-done is still delivered, so a poll that was running ends in one.
    /// Registering and dropping registrations go on working.
    ///
    /// Dropping the reader is what waits.
    pub fn shutdown(&self) {
        self.layer.shutdown();
    }

    /// The first error a run of the reader returned to its caller, if one has, taken. The reader
    /// goes on: the state is consistent after one, and the next wake runs it again.
    #[must_use]
    pub fn take_failure(&self) -> Option<io::Error> {
        self.layer.take_failure()
    }

    /// The layer, for the tests of this module to reach what the public items hide.
    #[cfg(test)]
    fn layer(&self) -> &ReadLayer<T> {
        &self.layer
    }
}

impl<T: Clone + Send + 'static> fmt::Debug for TailReader<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TailReader").finish_non_exhaustive()
    }
}

/// One file's registration with a [`TailReader`]. Dropping it unregisters the file.
///
/// It does not wait for anything: unregistering is legal at any time, including while a poll is
/// running. An entry already queued for the file stays queued, with its value, and the caller has
/// to cope with one arriving after the registration is gone. It may outlive the [`TailReader`] it
/// came from.
#[must_use = "dropping a registration unregisters it at once"]
pub struct TailRegistration<T: Clone + Send + 'static> {
    inner: Registration<Tracked<T>>,
}

impl<T: Clone + Send + 'static> TailRegistration<T> {
    /// A number for this registration: unique within its reader, handed out in order, never reused.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.inner.id()
    }

    /// A clone of the value the file was registered with. `T::clone` runs under the reader's
    /// internal lock, as the reader's documentation says.
    #[must_use]
    pub fn value(&self) -> T {
        self.inner.value().into_value()
    }

    /// The offset the file was registered at.
    #[must_use]
    pub fn start_offset(&self) -> u64 {
        self.inner.start_offset()
    }
}

impl<T: Clone + Send + 'static> fmt::Debug for TailRegistration<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TailRegistration")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}
