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

    pub(crate) fn set_state(&mut self, state: OperationState) {
        self.state = state;
    }

    /// Arm the reclaim thunk so rundown can free this operation generically.
    pub(crate) fn arm(&mut self) {
        self.reclaim = Some(reclaim_operation::<P>);
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
mod tests {
    use super::{Operation, OperationState};

    #[test]
    fn new_operation_is_idle_and_owns_its_payload() {
        let operation = Operation::new(vec![0u8; 8]);
        assert_eq!(operation.state(), OperationState::Idle);
        assert_eq!(operation.payload().len(), 8);
        assert_eq!(operation.into_payload(), vec![0u8; 8]);
    }

    #[test]
    fn overlapped_pointer_is_stable_and_distinct() {
        let operation = Operation::new(());
        assert!(!operation.overlapped_ptr().is_null());
        assert_eq!(operation.overlapped_ptr(), operation.overlapped_ptr());

        let other = Operation::new(());
        assert_ne!(operation.overlapped_ptr(), other.overlapped_ptr());
    }

    #[test]
    fn set_offset_splits_into_low_and_high_words() {
        let mut operation = Operation::new(());
        operation.set_offset(0x1_2345_6789);

        // SAFETY: no operation is in flight, so reading back the fields we just
        // wrote cannot race a kernel write.
        unsafe {
            let overlapped = &*operation.overlapped_ptr();
            assert_eq!(overlapped.Anonymous.Anonymous.Offset, 0x2345_6789);
            assert_eq!(overlapped.Anonymous.Anonymous.OffsetHigh, 0x1);
        }
    }
}
