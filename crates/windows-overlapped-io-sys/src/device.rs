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

use crate::operation::{payload_ptr_from_overlapped, sync_bytes_ptr_from_overlapped};
use crate::{
    AssociatedEndpoint, BlockingEndpoint, Completion, IoBuf, IoBufMut, Issued, Operation,
    OperationId, Started, Submitted,
};

impl BlockingEndpoint {
    /// Issue an overlapped `DeviceIoControl` with control code `code`, blocking
    /// until it completes.
    ///
    /// `input` is the input buffer (empty for control codes that take none) and
    /// `output` is the buffer the device writes into; the return value is how
    /// many bytes it wrote. Takes plain slices and allocates nothing: this call
    /// does not return until the operation is over, so an ordinary borrow
    /// provably covers the whole time the driver is using them.
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
        output: &mut [u8],
    ) -> io::Result<usize> {
        let in_len = checked_len(input.len(), "input")?;
        let out_len = checked_len(output.len(), "output")?;

        let in_ptr = in_ptr(input.as_ptr(), in_len);
        let out_ptr = out_ptr(output.as_mut_ptr(), out_len);

        let mut operation = Operation::new(());
        // SAFETY: issues exactly one DeviceIoControl reading `input` and writing
        // `output`, both valid for the whole blocking call; no other operation is
        // outstanding.
        unsafe {
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
        }
    }
}

/// The input buffer pointer, `NULL` for an empty buffer.
///
/// A control code that takes no input must be given `NULL` rather than a
/// dangling-but-nonnull pointer, which is what an empty buffer's `stable_ptr`
/// legitimately is.
fn in_ptr(ptr: *const u8, len: u32) -> *const c_void {
    if len == 0 {
        std::ptr::null()
    } else {
        ptr.cast()
    }
}

/// The output buffer pointer, `NULL` for an empty buffer.
fn out_ptr(ptr: *mut u8, len: u32) -> *mut c_void {
    if len == 0 {
        std::ptr::null_mut()
    } else {
        ptr.cast()
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
struct DeviceIoPayload<I, O> {
    input: I,
    output: O,
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
    /// `output` is the buffer the device writes its result into. Both are owned
    /// buffers of the caller's choosing, handed over for the operation's life
    /// and returned when it completes: nothing is copied and nothing is
    /// allocated here.
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
    pub unsafe fn ioctl<I: IoBuf, O: IoBufMut>(
        &self,
        code: u32,
        input: I,
        output: O,
    ) -> io::Result<Started<DeviceIoControlIo<I, O>, O>> {
        // The lengths are captured here rather than measured inside the
        // submission closure, which runs at the FFI boundary and cannot report
        // an error.
        let in_len = checked_len(input.bytes_len(), "input")?;
        let out_len = checked_len(output.bytes_len(), "output")?;
        let skip = self.notification_modes().skip_completion_port_on_success;

        let operation = Operation::new(DeviceIoPayload { input, output });
        // SAFETY: issues exactly one DeviceIoControl reading the payload's input
        // and writing its output, both reached through the pinned OVERLAPPED;
        // they and the byte-count cell live until the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<DeviceIoPayload<I, O>>(overlapped);
                let in_ptr = in_ptr((*payload).input.stable_ptr(), in_len);
                let out_ptr = out_ptr((*payload).output.stable_mut_ptr(), out_len);
                let bytes = sync_bytes_ptr_from_overlapped(overlapped);
                let ok = DeviceIoControl(
                    handle.as_raw_handle(),
                    code,
                    in_ptr,
                    in_len,
                    out_ptr,
                    out_len,
                    bytes,
                    overlapped,
                );
                classify_issued(ok, skip, bytes)
            })
        };
        finish_device(submitted)
    }
}

/// Map a native `BOOL` into the IOCP submission contract.
///
/// # Why an immediate `TRUE` is usually `Pending`
///
/// [`Issued`] does not record whether `DeviceIoControl` finished synchronously.
/// It records whether a **completion packet will arrive on the port**, and for
/// an overlapped handle bound to an IOCP those are different facts: the I/O
/// Manager queues a packet for every request it completes, *including* one that
/// succeeds immediately without returning `ERROR_IO_PENDING`. See
/// [`Issued::Pending`] for the full statement of that rule.
///
/// The single exception is `skip_on_success`, which is why this needs to know
/// it: on an endpoint in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode no packet
/// is queued for an immediate success, so that -- and only that -- is an
/// [`Issued::Completed`]. Both directions of getting this wrong are serious.
/// Answering `Completed` when a packet is coming tells the port to reclaim the
/// operation's storage inline, and the packet then arrives carrying a dangling
/// `OVERLAPPED` -- a use-after-free on claim. Answering `Pending` when none is
/// coming leaves the operation counted as outstanding forever, so
/// [`crate::CompletionPort::run_down`] spins waiting for a packet that will
/// never be queued.
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

/// Turn a device submission outcome into the adapter's two-state outcome.
fn finish_device<I: IoBuf, O: IoBufMut>(
    submitted: Submitted<DeviceIoPayload<I, O>>,
) -> io::Result<Started<DeviceIoControlIo<I, O>, O>> {
    match submitted {
        Submitted::Pending(id) => Ok(Started::Pending(DeviceIoControlIo {
            id,
            buffers: std::marker::PhantomData,
        })),
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
/// The token carries the operation's identity and remembers both buffer types it
/// was submitted with, so [`DeviceIoControlIo::claim`] hands back the caller's
/// own output buffer -- the same value, not a copy -- once the matching
/// completion is dequeued. The input type is carried too, because it is part of
/// the payload type the claim must name.
#[derive(Debug)]
pub struct DeviceIoControlIo<I, O> {
    id: OperationId,
    /// The buffers live in the pinned operation, not here; this only keeps the
    /// token's type tied to them so `claim` cannot be handed the wrong payload.
    buffers: std::marker::PhantomData<fn() -> (I, O)>,
}

impl<I: IoBuf, O: IoBufMut> DeviceIoControlIo<I, O> {
    /// The identity of the in-flight operation, for cancellation or matching.
    #[must_use]
    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Claim this operation's result from `completion`.
    ///
    /// On a match returns `Ok((output, result))`: `output` is the buffer the
    /// caller handed over (valid up to the byte count) and `result` is the byte
    /// count or the operation's error. Returns `Err(self)` when `completion`
    /// belongs to a different operation.
    ///
    /// The input buffer is dropped here: the driver is done reading it, and
    /// returning both would make the common case pay for the rare one.
    pub fn claim(self, completion: &Completion) -> Result<(O, io::Result<usize>), Self> {
        if completion.id() != Some(self.id) {
            return Err(self);
        }
        // SAFETY: the full identity -- address *and* generation -- matches, which
        // an address alone would not: a recycled address can belong to a later
        // operation of a different payload type. The match therefore proves this
        // completion is the Operation<DeviceIoPayload<I, O>> this token
        // submitted, and the token's own type parameters name that payload;
        // claim it exactly once.
        let operation = unsafe { completion.claim::<DeviceIoPayload<I, O>>() };
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
