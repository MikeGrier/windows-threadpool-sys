// Copyright (c) 2026 Mike Grier
//! Buffer-owning device-control operation adapters, gated behind the `device`
//! feature.
//!
//! These wrappers own the input and output buffers and issue the single native
//! `DeviceIoControl` internally, so a caller performs an overlapped device
//! control without touching `OVERLAPPED` or the submission seam. A device is a
//! `HANDLE`, so the adapters extend the existing handle endpoints rather than
//! introducing a device-specific type.
//!
//! The `ioctl` methods are `unsafe` because they take an arbitrary control code:
//! a code whose input structure embeds raw pointers to separate buffers (such as
//! `SCSI_PASS_THROUGH_DIRECT`) reaches storage these adapters do not own, so only
//! the caller can guarantee that storage outlives the operation. A self-contained
//! code -- an `FSCTL` query, say -- needs nothing beyond the owned buffers, but
//! the seam cannot tell the two apart, so the obligation lives in the contract.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::operation::payload_ptr_from_overlapped;
use crate::{
    AssociatedEndpoint, BlockingEndpoint, Completion, Issued, Operation, OperationId, Started,
    Submitted,
};

impl BlockingEndpoint {
    /// Issue an overlapped `DeviceIoControl` with control code `code`, blocking
    /// until it completes.
    ///
    /// `input` is the input buffer (empty for control codes that take none) and
    /// `output_len` sizes the output buffer. Returns the output truncated to the
    /// bytes returned and that count.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if either buffer is longer than
    /// `u32::MAX` bytes, which the control code's byte counts cannot express, or
    /// any error from issuing or completing the control operation.
    ///
    /// # Safety
    ///
    /// `code`'s input layout must be *self-contained*: the driver may read or
    /// write only the bytes inside `input` and the output buffer, never memory
    /// reached through a pointer embedded in `input`. Some control codes take an
    /// input structure that carries raw pointers to separate buffers --
    /// `SCSI_PASS_THROUGH_DIRECT::DataBuffer` is one -- which this adapter neither
    /// owns nor keeps alive. For such a code the caller must keep every referenced
    /// buffer valid for the whole call; the adapter cannot, because it does not
    /// know the code's layout. This is why the generic raw-code seam is `unsafe`
    /// even though a self-contained code (an `FSCTL` query, say) needs nothing
    /// more than owned buffers.
    pub unsafe fn ioctl(
        &mut self,
        code: u32,
        input: &[u8],
        output_len: usize,
    ) -> io::Result<(Vec<u8>, usize)> {
        // Checked before allocating, so an unusable request costs nothing.
        let in_len = checked_len(input.len(), "input")?;
        let out_len = checked_len(output_len, "output")?;

        let mut output = vec![0_u8; output_len];
        let in_ptr = in_ptr(input);
        let out_ptr = out_ptr(&mut output);

        let mut operation = Operation::new(());
        // SAFETY: issues exactly one DeviceIoControl reading `input` and writing
        // `output`, both valid for the whole blocking call; no other operation is
        // outstanding.
        let returned = unsafe {
            self.run(&mut operation, |handle, overlapped| {
                let ok = DeviceIoControl(
                    handle.as_raw_handle(),
                    code,
                    in_ptr,
                    in_len,
                    out_ptr,
                    out_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify(ok)
            })
        }?;

        output.truncate(returned);
        Ok((output, returned))
    }
}

/// The input buffer pointer, `NULL` for an empty buffer.
fn in_ptr(input: &[u8]) -> *const c_void {
    if input.is_empty() {
        std::ptr::null()
    } else {
        input.as_ptr().cast()
    }
}

/// The output buffer pointer, `NULL` for an empty buffer.
fn out_ptr(output: &mut [u8]) -> *mut c_void {
    if output.is_empty() {
        std::ptr::null_mut()
    } else {
        output.as_mut_ptr().cast()
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

/// Convert a buffer length to the `u32` byte count `DeviceIoControl` takes.
///
/// Rejects rather than caps. Capping would submit a prefix of the caller's
/// input, or tell the device an output buffer is smaller than it is, and report
/// success for an operation that did something other than what was asked.
fn checked_len(len: usize, which: &str) -> io::Result<u32> {
    u32::try_from(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a DeviceIoControl {which} buffer is limited to u32::MAX bytes; {len} does not fit"
            ),
        )
    })
}

/// The pinned payload for an in-flight device-control operation: the input and
/// output buffers, both of which must outlive the async call.
struct DeviceIoPayload {
    input: Vec<u8>,
    output: Vec<u8>,
}

impl AssociatedEndpoint<'_> {
    /// Submit an overlapped `DeviceIoControl` with control code `code`.
    ///
    /// Returns [`Started::Pending`] with a [`DeviceIoControlIo`] token that
    /// recovers the output buffer and byte count from the operation's
    /// completion, or -- only on an endpoint in
    /// `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode, where a synchronous success
    /// queues no packet -- [`Started::Completed`] with the output buffer already
    /// in hand.
    ///
    /// `input` is the input buffer (empty for control codes that take none) and
    /// `output_len` sizes the output buffer.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if either buffer is longer than
    /// `u32::MAX` bytes, which the control code's byte counts cannot express, or
    /// any immediate failure from issuing the control operation.
    ///
    /// # Safety
    ///
    /// `code`'s input layout must be *self-contained*: the driver may read or
    /// write only the bytes inside `input` and the output buffer, never memory
    /// reached through a pointer embedded in `input`. Some control codes take an
    /// input structure that carries raw pointers to separate buffers --
    /// `SCSI_PASS_THROUGH_DIRECT::DataBuffer` is one -- which this adapter neither
    /// owns nor keeps alive. Because the operation outlives this call, such a
    /// pointee could be freed while the driver is still using it. For such a code
    /// the caller must keep every referenced buffer alive until the operation
    /// completes; the adapter cannot, because it does not know the code's layout.
    /// This is why the generic raw-code seam is `unsafe` even though a
    /// self-contained code (an `FSCTL` query, say) needs nothing more than owned
    /// buffers.
    #[track_caller]
    pub unsafe fn ioctl(
        &self,
        code: u32,
        input: Vec<u8>,
        output_len: usize,
    ) -> io::Result<Started<DeviceIoControlIo, Vec<u8>>> {
        // Checked before allocating, so an unusable request costs nothing. The
        // lengths are captured here rather than measured inside the submission
        // closure, which runs at the FFI boundary and cannot report an error.
        let in_len = checked_len(input.len(), "input")?;
        let out_len = checked_len(output_len, "output")?;

        let operation = Operation::new(DeviceIoPayload {
            input,
            output: vec![0_u8; output_len],
        });
        // SAFETY: issues exactly one DeviceIoControl reading the payload's input
        // and writing its output, both reached through the pinned OVERLAPPED;
        // they live until the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<DeviceIoPayload>(overlapped);
                let in_ptr = in_ptr(&(*payload).input);
                let out_ptr = out_ptr(&mut (*payload).output);
                let ok = DeviceIoControl(
                    handle.as_raw_handle(),
                    code,
                    in_ptr,
                    in_len,
                    out_ptr,
                    out_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify_issued(ok)
            })
        };
        finish_device(submitted)
    }
}

/// Map a native `BOOL` into the IOCP submission contract, expecting a completion
/// packet on success because this adapter is only used on endpoints that are not
/// in skip-on-success mode.
///
/// # Why an immediate `TRUE` is `Pending` and not `Completed`
///
/// [`Issued`] does not record whether `DeviceIoControl` finished synchronously.
/// It records whether a **completion packet will arrive on the port**, and for
/// an overlapped handle bound to an IOCP those are different facts: the I/O
/// Manager queues a packet for every request it completes, *including* one that
/// succeeds immediately without returning `ERROR_IO_PENDING`. The one documented
/// exception is `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`. See [`Issued::Pending`]
/// for the full statement of that rule.
///
/// So `ok != 0` means the *I/O* is already done -- and its packet is already
/// queued, waiting to be claimed. That is exactly `Pending`: the kernel still
/// holds the operation's storage, and [`Completion::claim`] is still what
/// recovers the output buffer and byte count. Answering `Completed` here would
/// tell the port to reclaim that storage inline while a packet carrying the
/// same `OVERLAPPED` was still in flight -- a use-after-free on claim, and a
/// rundown that returns while an operation is still outstanding.
///
/// # Skip-on-success is supported by the core, just not by this adapter
///
/// This is a limitation of the *buffer-owning adapter*, not of the crate. The
/// submission seam handles skip-on-success fully -- that is what
/// [`Issued::Completed`] and [`Submitted::Completed`] are for, and both backends
/// implement the inline-reclaim path for it. What [`AssociatedEndpoint::ioctl`]
/// cannot express is the *result*: it hands back a [`DeviceIoControlIo`] token
/// whose entire contract is "claim me from a completion later", and there is no
/// room in that shape for "already done, here is your output buffer". So
/// [`finish_device`] reports a synchronous completion as an error rather than
/// silently losing the result. Widening the adapters to return an
/// already-complete result is a deliberate, still-unbuilt piece of work, not a
/// judgement that the flag is unwanted.
fn classify_issued(ok: i32) -> io::Result<Issued> {
    // Both success shapes are `Pending` for the same reason: a packet is coming
    // either way. `TRUE` means it is queued already; `ERROR_IO_PENDING` means it
    // is queued once the request finishes. The port cannot distinguish them from
    // the packet alone, and has no need to.
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

/// Turn a device submission outcome into the adapter's two-state outcome.
fn finish_device(
    submitted: Submitted<DeviceIoPayload>,
) -> io::Result<Started<DeviceIoControlIo, Vec<u8>>> {
    match submitted {
        Submitted::Pending(id) => Ok(Started::Pending(DeviceIoControlIo { id })),
        Submitted::Completed {
            operation,
            bytes_transferred,
        } => Ok(Started::Completed {
            payload: operation.into_payload().output,
            bytes_transferred: bytes_transferred as usize,
        }),
        Submitted::Failed { error, .. } => Err(error),
    }
}

/// A pending device-control operation submitted through
/// [`AssociatedEndpoint::ioctl`].
///
/// The token carries the operation's identity and its payload type, so
/// [`DeviceIoControlIo::claim`] recovers the output buffer and byte count safely
/// once the matching completion is dequeued.
#[derive(Debug)]
pub struct DeviceIoControlIo {
    id: OperationId,
}

impl DeviceIoControlIo {
    /// The identity of the in-flight operation, for cancellation or matching.
    #[must_use]
    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Claim this operation's result from `completion`.
    ///
    /// On a match returns `Ok((output, result))`: `output` is the output buffer
    /// (valid up to the byte count) and `result` is the byte count or the
    /// operation's error. Returns `Err(self)` when `completion` belongs to a
    /// different operation.
    pub fn claim(self, completion: &Completion) -> Result<(Vec<u8>, io::Result<usize>), Self> {
        if completion.id() != Some(self.id) {
            return Err(self);
        }
        // SAFETY: the full identity -- address *and* generation -- matches, which
        // an address alone would not: a recycled address can belong to a later
        // operation of a different payload type. The match therefore proves this
        // completion is the
        // Operation<DeviceIoPayload> this token submitted; claim it exactly once.
        let operation = unsafe { completion.claim::<DeviceIoPayload>() };
        let output = operation.into_payload().output;
        let result = match completion.error() {
            Some(error) => Err(io::Error::from_raw_os_error(
                error.raw_os_error().unwrap_or_default(),
            )),
            None => Ok(completion.bytes_transferred() as usize),
        };
        Ok((output, result))
    }
}

#[cfg(test)]
mod tests;
