// Copyright (c) 2026 Mike Grier
//! The reading layer's buffers: a fixed set of owned buffers of one size, lent one at a time and
//! given back when the bytes read into them are no longer wanted (TW-D-15).
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the reading engine (TW-5.5) takes and lends them")
)]

use std::collections::TryReserveError;
use std::ops::Deref;
use std::os::windows::io::BorrowedHandle;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::{fmt, io, mem};

use io_buffers::{IoBuf, IoBufMut};

use crate::doorbell::Doorbell;

#[cfg(test)]
mod tests;

/// The longest buffer a ring read takes: its length is a `u32`.
pub(crate) const MAX_BUFFER_LEN: usize = u32::MAX as usize;

/// Why a pool could not be made.
#[derive(Debug)]
pub(crate) enum PoolError {
    /// A buffer of no bytes, or of more than [`MAX_BUFFER_LEN`].
    BufferLen(usize),
    /// A pool of no buffers.
    NoBuffers,
    /// The buffers could not be allocated.
    Allocation(TryReserveError),
    /// The doorbell could not be created.
    Io(io::Error),
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferLen(len) => write!(
                f,
                "a buffer of {len} bytes; it must be of 1 to {MAX_BUFFER_LEN} bytes"
            ),
            Self::NoBuffers => f.write_str("a pool of no buffers"),
            Self::Allocation(error) => {
                write!(f, "the pool's buffers could not be allocated: {error}")
            }
            Self::Io(error) => write!(f, "the pool's doorbell could not be created: {error}"),
        }
    }
}

impl std::error::Error for PoolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BufferLen(_) | Self::NoBuffers => None,
            Self::Allocation(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

/// `count` owned buffers of `buffer_len` bytes, all allocated when the pool is made.
///
/// [`take`](Self::take) lends one as a [`PoolBuffer`], which goes back when it drops, whether it was
/// made into a [`Chunk`] or not. The pool never allocates past its count: when every buffer is lent,
/// `take` refuses, and the next buffer given back rings [`doorbell`](Self::doorbell). Lent buffers
/// may outlive the pool.
#[derive(Debug)]
pub(crate) struct BufferPool {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    buffer_len: usize,
    count: usize,
    free: Mutex<Free>,
    // Rung when a buffer comes back to a pool that refused a take since it was last rung.
    doorbell: Doorbell,
}

#[derive(Debug)]
struct Free {
    buffers: Vec<Box<[u8]>>,
    // A take was refused, and no buffer has come back since.
    starved: bool,
}

impl Shared {
    fn free(&self) -> MutexGuard<'_, Free> {
        // Nothing under the lock can leave the free list half-changed.
        self.free.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn give_back(&self, bytes: Box<[u8]>) {
        let was_starved = {
            let mut free = self.free();
            free.buffers.push(bytes);
            mem::replace(&mut free.starved, false)
        };
        if was_starved {
            self.doorbell.ring();
        }
    }
}

/// Check a pool's dimensions before anything is allocated.
fn check(buffer_len: usize, count: usize) -> Result<(), PoolError> {
    if buffer_len == 0 || buffer_len > MAX_BUFFER_LEN {
        return Err(PoolError::BufferLen(buffer_len));
    }
    if count == 0 {
        return Err(PoolError::NoBuffers);
    }
    Ok(())
}

fn zeroed(len: usize) -> Result<Box<[u8]>, TryReserveError> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len)?;
    bytes.resize(len, 0);
    Ok(bytes.into_boxed_slice())
}

impl BufferPool {
    /// A pool of `count` buffers of `buffer_len` bytes each, every byte zeroed.
    ///
    /// # Errors
    ///
    /// [`PoolError::BufferLen`] and [`PoolError::NoBuffers`] before anything is allocated;
    /// [`PoolError::Allocation`] when the memory is refused; [`PoolError::Io`] when the doorbell
    /// cannot be created.
    pub(crate) fn new(buffer_len: usize, count: usize) -> Result<Self, PoolError> {
        check(buffer_len, count)?;
        let mut buffers = Vec::new();
        buffers
            .try_reserve_exact(count)
            .map_err(PoolError::Allocation)?;
        for _ in 0..count {
            buffers.push(zeroed(buffer_len).map_err(PoolError::Allocation)?);
        }
        Ok(Self {
            shared: Arc::new(Shared {
                buffer_len,
                count,
                free: Mutex::new(Free {
                    buffers,
                    starved: false,
                }),
                doorbell: Doorbell::new().map_err(PoolError::Io)?,
            }),
        })
    }

    /// How many bytes each buffer holds.
    pub(crate) fn buffer_len(&self) -> usize {
        self.shared.buffer_len
    }

    /// How many buffers the pool has, lent or not.
    pub(crate) fn count(&self) -> usize {
        self.shared.count
    }

    /// How many buffers are not lent.
    pub(crate) fn available(&self) -> usize {
        self.shared.free().buffers.len()
    }

    /// Lend a buffer, or refuse when every one is lent; after a refusal, the next buffer given
    /// back rings [`doorbell`](Self::doorbell).
    pub(crate) fn take(&self) -> Option<PoolBuffer> {
        let mut free = self.shared.free();
        match free.buffers.pop() {
            Some(bytes) => Some(PoolBuffer {
                window: bytes.len(),
                bytes,
                shared: Arc::clone(&self.shared),
            }),
            None => {
                free.starved = true;
                None
            }
        }
    }

    /// The event rung when a buffer comes back after a refused [`take`](Self::take). Manual-reset:
    /// reset it before taking again, so a buffer given back after the reset rings it again.
    pub(crate) fn doorbell(&self) -> BorrowedHandle<'_> {
        self.shared.doorbell.handle()
    }

    /// Clear [`doorbell`](Self::doorbell).
    pub(crate) fn reset_doorbell(&self) {
        self.shared.doorbell.reset();
    }
}

/// One of a pool's buffers, lent. It goes back to its pool when it drops.
///
/// It is what a ring read is given to read into, so it keeps the buffer's address for its life:
/// the bytes are a heap allocation that moving this value does not move.
pub(crate) struct PoolBuffer {
    // Always the full buffer, until `drop` takes it to give back.
    bytes: Box<[u8]>,
    // How many of its first bytes a read is asked to fill: all of them until `limit_to`.
    window: usize,
    shared: Arc<Shared>,
}

impl PoolBuffer {
    /// Ask a read for only the first `len` bytes, so it cannot read past what was wanted.
    ///
    /// # Panics
    ///
    /// When `len` is 0 or longer than the buffer.
    pub(crate) fn limit_to(&mut self, len: usize) {
        assert!(
            (1..=self.bytes.len()).contains(&len),
            "a window of {len} bytes in a buffer of {}",
            self.bytes.len()
        );
        self.window = len;
    }

    /// Keep the first `len` bytes as what was read.
    ///
    /// # Panics
    ///
    /// When `len` is longer than the window.
    pub(crate) fn into_chunk(self, len: usize) -> Chunk {
        assert!(
            len <= self.window,
            "{len} bytes read into a window of {}",
            self.window
        );
        Chunk { buffer: self, len }
    }
}

impl Drop for PoolBuffer {
    fn drop(&mut self) {
        self.shared.give_back(mem::take(&mut self.bytes));
    }
}

impl fmt::Debug for PoolBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolBuffer")
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

// SAFETY: the bytes are a boxed slice, whose heap address moving this value does not change; its
// length is fixed, and nothing replaces it until `drop`, which runs only once the I/O layer has
// handed the value back. Every byte is initialized: the pool zeroes them when it allocates.
unsafe impl IoBuf for PoolBuffer {
    fn stable_ptr(&self) -> *const u8 {
        self.bytes.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.window
    }
}

// SAFETY: as `IoBuf`; the box is this value's alone, so `&mut self` is exclusive access, and
// `as_mut_ptr` is the address `as_ptr` reports.
unsafe impl IoBufMut for PoolBuffer {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.bytes.as_mut_ptr()
    }
}

/// The bytes one read delivered, in a buffer lent from the reading layer's pool. Dropping it gives
/// the buffer back.
pub(crate) struct Chunk {
    buffer: PoolBuffer,
    len: usize,
}

impl Deref for Chunk {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.buffer.bytes[..self.len]
    }
}

impl AsRef<[u8]> for Chunk {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunk")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}
