// Copyright (c) 2026 Mike Grier
//! `Batch`: a scoped, exclusive submission window (M3.1-M3.3, M3.5).

use std::ffi::c_void;
use std::io;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingCancelRequest, BuildIoRingFlushFile, BuildIoRingReadFile, BuildIoRingWriteFile,
    FILE_FLUSH_DEFAULT, FILE_WRITE_FLAGS_NONE, IORING_BUFFER_REF, IORING_BUFFER_REF_0,
    IORING_HANDLE_REF, IORING_HANDLE_REF_0, IORING_REF_RAW, IORING_SQE_FLAGS,
    IOSQE_FLAGS_DRAIN_PRECEDING_OPS, IOSQE_FLAGS_NONE, SubmitIoRing,
};

use crate::buf::{IoBuf, IoBufMut};
use crate::error::check;
use crate::ring::{IoRing, Op};
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

fn raw_handle_ref(file: HANDLE) -> IORING_HANDLE_REF {
    IORING_HANDLE_REF {
        Kind: IORING_REF_RAW,
        Handle: IORING_HANDLE_REF_0 { Handle: file },
    }
}

fn raw_buffer_ref(address: *mut c_void) -> IORING_BUFFER_REF {
    IORING_BUFFER_REF {
        Kind: IORING_REF_RAW,
        Buffer: IORING_BUFFER_REF_0 { Address: address },
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
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the ring was not probed as
    /// supporting [`Op::Read`]; [`io::ErrorKind::InvalidInput`] if the
    /// buffer is longer than `u32::MAX`; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room (M3.3, not
    /// auto-flushed -- see [`Batch`]'s own docs); or any other error from
    /// `BuildIoRingReadFile`. On any error the buffer is dropped normally,
    /// not leaked or handed back.
    pub fn read<B: IoBufMut>(
        &mut self,
        file: HANDLE,
        mut buffer: B,
        offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<B>> {
        self.require(Op::Read)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_mut_ptr().cast::<c_void>();
        let token = Token::new(self.ring, buffer)?;
        let user_data = token.id();
        // SAFETY: `self.ring`'s handle is live; `address` is `IoBufMut`'s
        // promised stable, exclusively-owned pointer, valid for `len` bytes
        // until `token` is claimed; `file` is the caller's to keep alive.
        let hr = unsafe {
            BuildIoRingReadFile(
                self.ring.raw_handle(),
                raw_handle_ref(file),
                raw_buffer_ref(address),
                len,
                offset,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token, user_data)
    }

    /// Queue a write of `buffer.bytes_len()` bytes to `file` at `offset`.
    ///
    /// # Errors
    ///
    /// As [`Batch::read`], plus any error from `BuildIoRingWriteFile`.
    pub fn write<B: IoBuf>(
        &mut self,
        file: HANDLE,
        buffer: B,
        offset: u64,
        options: PushOptions,
    ) -> io::Result<Token<B>> {
        self.require(Op::Write)?;
        let len = checked_len(buffer.bytes_len())?;
        let address = buffer.stable_ptr().cast_mut().cast::<c_void>();
        let token = Token::new(self.ring, buffer)?;
        let user_data = token.id();
        // SAFETY: `address` is `IoBuf`'s promised stable pointer, valid for
        // `len` bytes until `token` is claimed; the kernel only reads
        // through it for a write, so the cast away from `const` does not
        // authorize mutation. `file` is the caller's to keep alive.
        let hr = unsafe {
            BuildIoRingWriteFile(
                self.ring.raw_handle(),
                raw_handle_ref(file),
                raw_buffer_ref(address),
                len,
                offset,
                FILE_WRITE_FLAGS_NONE,
                user_data,
                options.sqe_flags(),
            )
        };
        self.finish_push(hr, token, user_data)
    }

    /// Reclaim `token`'s buffer and release its reservation if `hr` failed,
    /// or hand `token` back unchanged on success -- the shared tail of
    /// [`Batch::read`] and [`Batch::write`].
    fn finish_push<B: IoBuf>(
        &mut self,
        hr: windows_sys::core::HRESULT,
        token: Token<B>,
        user_data: usize,
    ) -> io::Result<Token<B>> {
        match check(hr) {
            Ok(()) => Ok(token),
            Err(error) => {
                // The SQE was never queued: reclaim and drop the buffer
                // normally instead of leaking it (claiming a token by its
                // own id always succeeds), and release the reservation so
                // it does not count against rundown.
                let _ = token.claim_if(user_data);
                self.ring.cancel_reservation();
                Err(error)
            }
        }
    }

    /// Queue a flush of `file`'s buffered data (`FILE_FLUSH_DEFAULT`).
    ///
    /// There is no buffer, so this returns the raw `UserData` identity
    /// rather than a [`Token`]: nothing owns a buffer for a completion to
    /// hand back.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the ring was not probed as
    /// supporting [`Op::Flush`]; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room; or any
    /// other error from `BuildIoRingFlushFile`.
    pub fn flush(&mut self, file: HANDLE, options: PushOptions) -> io::Result<usize> {
        self.require(Op::Flush)?;
        let user_data = self.ring.reserve_user_data()?;
        // SAFETY: `self.ring`'s handle is live; `file` is the caller's to
        // keep alive; there is no buffer.
        let hr = unsafe {
            BuildIoRingFlushFile(
                self.ring.raw_handle(),
                raw_handle_ref(file),
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

    /// Queue cancellation of the operation identified by `target` (the
    /// `usize` a prior push returned), against `file`.
    ///
    /// A cancel is itself an operation: it completes on its own `UserData`,
    /// returned here, independently of whether `target` was actually
    /// outstanding. Cancelling a target that has already completed -- or
    /// was never outstanding -- reports `ERROR_NOT_FOUND` through *this*
    /// completion rather than failing to build (M3.6).
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the ring was not probed as
    /// supporting [`Op::Cancel`]; an [`crate::IoRingError`] wrapping
    /// `IORING_E_SUBMISSION_QUEUE_FULL` if the queue has no room; or any
    /// other error from `BuildIoRingCancelRequest`.
    pub fn cancel(&mut self, file: HANDLE, target: usize) -> io::Result<usize> {
        self.require(Op::Cancel)?;
        let user_data = self.ring.reserve_user_data()?;
        // SAFETY: `self.ring`'s handle is live; `file` is the caller's to
        // keep alive; `BuildIoRingCancelRequest` takes no SQE-flags
        // parameter.
        let hr = unsafe {
            BuildIoRingCancelRequest(
                self.ring.raw_handle(),
                raw_handle_ref(file),
                target,
                user_data,
            )
        };
        if let Err(error) = check(hr) {
            self.ring.cancel_reservation();
            return Err(error);
        }
        Ok(user_data)
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
        check(hr)?;
        self.submitted = true;
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
