// Copyright (c) 2026 Mike Grier
//! Pinned per-operation storage: an `OVERLAPPED` coupled to its payload.
//!
//! Each in-flight overlapped operation owns stable storage holding its
//! `OVERLAPPED`, the caller's opaque payload, and an explicit lifecycle state.
//! The address of the `OVERLAPPED` is the completion identity a backend uses to
//! match a dequeued packet back to its operation, so the storage must not move
//! or be reused while an operation is outstanding. This module models the
//! storage and identity only; submission, completion, and reclamation belong to
//! the individual completion backends.

use std::cell::UnsafeCell;

use windows_sys::Win32::System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};

/// The lifecycle state of an overlapped operation's storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationState {
    /// Constructed but not yet submitted; the payload may still be prepared.
    Idle,
    /// Handed to a native submission call whose outcome is not yet known.
    Submitted,
    /// Accepted by the kernel and awaiting an asynchronous completion.
    Pending,
    /// A completion has been observed; the payload and result may be read.
    Completed,
    /// Cancellation was requested before completion was observed.
    Cancelled,
}

/// Stable storage for one overlapped operation.
///
/// `P` is the caller's payload, for example a buffer or a descriptor array. The
/// crate never interprets it. The `OVERLAPPED` lives in an [`UnsafeCell`] because
/// the kernel writes to it through [`Operation::overlapped_ptr`] while the owner
/// holds only a shared reference.
// `repr(C)` with `overlapped` first keeps the operation pointer identical to its
// `OVERLAPPED` pointer, so a completion can recover the operation from it. The
// `reclaim` thunk sits before `payload`, so its offset is the same for every `P`
// and can be read from the `OVERLAPPED` pointer alone during rundown.
#[derive(Debug)]
#[repr(C)]
pub struct Operation<P> {
    overlapped: UnsafeCell<OVERLAPPED>,
    // Read only through `reclaim_from_overlapped`, via its fixed offset.
    #[allow(dead_code)]
    reclaim: Option<unsafe fn(*mut OVERLAPPED)>,
    state: OperationState,
    payload: P,
}

/// Offset of the `reclaim` field, identical for every `P`.
const RECLAIM_OFFSET: usize = core::mem::offset_of!(Operation<()>, reclaim);

/// Drop a leaked `Box<Operation<P>>` given its `OVERLAPPED` pointer.
///
/// # Safety
///
/// `overlapped` must be the base of a live `Box<Operation<P>>` reclaimed exactly
/// once.
pub(crate) unsafe fn reclaim_operation<P>(overlapped: *mut OVERLAPPED) {
    drop(unsafe { Box::from_raw(overlapped.cast::<Operation<P>>()) });
}

/// Run the reclaim thunk armed on the operation identified by `overlapped`.
///
/// # Safety
///
/// `overlapped` must be the identity pointer of a live armed operation reclaimed
/// exactly once.
pub(crate) unsafe fn reclaim_from_overlapped(overlapped: *mut OVERLAPPED) {
    let slot = unsafe {
        overlapped
            .cast::<u8>()
            .add(RECLAIM_OFFSET)
            .cast::<Option<unsafe fn(*mut OVERLAPPED)>>()
    };
    if let Some(reclaim) = unsafe { *slot } {
        unsafe { reclaim(overlapped) };
    }
}

/// Recover a pointer to the payload of an operation from its `OVERLAPPED`
/// identity.
///
/// The payload offset depends on `P`, so the caller must supply the exact `P`
/// the operation was created with. It is used by a family adapter to reach the
/// buffer it owns inside the pinned operation while issuing the native call.
///
/// # Safety
///
/// `overlapped` must be the identity pointer of a live `Operation<P>` of this
/// exact type, and the returned pointer must be used only while that operation's
/// storage stays put and nothing else accesses the payload concurrently.
#[cfg(any(feature = "fs", feature = "socket"))]
pub(crate) unsafe fn payload_ptr_from_overlapped<P>(overlapped: *mut OVERLAPPED) -> *mut P {
    let offset = core::mem::offset_of!(Operation<P>, payload);
    unsafe { overlapped.cast::<u8>().add(offset).cast::<P>() }
}

/// Reclaim and drop an operation from its `OVERLAPPED` identity without knowing
/// its payload type, using the thunk armed by [`Operation::into_overlapped`].
///
/// This lets a backend free operations of mixed payload types during rundown.
///
/// # Safety
///
/// `overlapped` must have been returned by [`Operation::into_overlapped`] and
/// must be reclaimed exactly once.
pub unsafe fn reclaim_overlapped(overlapped: *mut OVERLAPPED) {
    unsafe { reclaim_from_overlapped(overlapped) };
}

impl<P> Operation<P> {
    /// Create idle storage with a zeroed `OVERLAPPED` and the given payload.
    #[must_use]
    pub fn new(payload: P) -> Self {
        let overlapped = OVERLAPPED {
            Internal: 0,
            InternalHigh: 0,
            Anonymous: OVERLAPPED_0 {
                Anonymous: OVERLAPPED_0_0 {
                    Offset: 0,
                    OffsetHigh: 0,
                },
            },
            hEvent: std::ptr::null_mut(),
        };
        Self {
            overlapped: UnsafeCell::new(overlapped),
            reclaim: None,
            state: OperationState::Idle,
            payload,
        }
    }

    /// Return the current lifecycle state.
    #[must_use]
    pub fn state(&self) -> OperationState {
        self.state
    }

    /// Set the lifecycle state marker.
    pub fn set_state(&mut self, state: OperationState) {
        self.state = state;
    }

    /// Arm the reclaim thunk so rundown can free this operation generically.
    pub(crate) fn arm(&mut self) {
        self.reclaim = Some(reclaim_operation::<P>);
    }

    /// Consume the operation for submission, transferring ownership out and
    /// returning its stable `OVERLAPPED` identity.
    ///
    /// The returned pointer identifies the operation and must be handed to
    /// exactly one native overlapped call. Recover the operation afterward with
    /// [`Operation::from_overlapped`] when the payload type is known (as in a
    /// completion), or with [`reclaim_overlapped`] when it is not (as during
    /// rundown). This is the submission seam shared by the completion-port and
    /// thread-pool backends.
    #[must_use]
    pub fn into_overlapped(mut self) -> *mut OVERLAPPED {
        self.arm();
        self.state = OperationState::Pending;
        let boxed = Box::new(self);
        let overlapped = boxed.overlapped_ptr();
        // Ownership transfers to the caller until the operation is reclaimed.
        let _ = Box::into_raw(boxed);
        overlapped
    }

    /// Recover an operation previously submitted with [`Operation::into_overlapped`].
    ///
    /// # Safety
    ///
    /// `overlapped` must have been returned by [`Operation::into_overlapped`] on
    /// an `Operation<P>` of this exact type, and must be reclaimed exactly once.
    #[must_use]
    pub unsafe fn from_overlapped(overlapped: *mut OVERLAPPED) -> Self {
        unsafe { *Box::from_raw(overlapped.cast::<Operation<P>>()) }
    }

    /// Borrow the payload.
    #[must_use]
    pub fn payload(&self) -> &P {
        &self.payload
    }

    /// Mutably borrow the payload.
    ///
    /// Exclusive access proves no operation is in flight, so preparing or
    /// inspecting the payload here cannot race a kernel write.
    #[must_use]
    pub fn payload_mut(&mut self) -> &mut P {
        &mut self.payload
    }

    /// Consume the storage and recover the payload.
    #[must_use]
    pub fn into_payload(self) -> P {
        self.payload
    }

    /// Set the seek position for endpoints that use `OVERLAPPED` offsets.
    ///
    /// Non-seekable endpoints such as pipes and sockets must leave the offset at
    /// its default of zero.
    pub fn set_offset(&mut self, offset: u64) {
        let overlapped = self.overlapped.get_mut();
        // The offset fields share a union with the unused `Pointer` member.
        overlapped.Anonymous.Anonymous.Offset = offset as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
    }

    /// Return the stable pointer that identifies this operation to a backend.
    ///
    /// The pointer is valid only while the storage stays put; a backend must pin
    /// the operation before submitting and must not free the storage until the
    /// matching completion has been observed.
    #[must_use]
    pub fn overlapped_ptr(&self) -> *mut OVERLAPPED {
        self.overlapped.get()
    }
}

#[cfg(test)]
mod tests;
