// Copyright (c) 2026 Mike Grier
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

#[test]
fn into_and_from_overlapped_round_trip() {
    let operation = Operation::new(vec![1_u8, 2, 3]);
    let overlapped = operation.into_overlapped();
    assert!(!overlapped.is_null());

    // SAFETY: reclaim the exact operation just leaked, exactly once.
    let recovered = unsafe { Operation::<Vec<u8>>::from_overlapped(overlapped) };
    assert_eq!(recovered.state(), OperationState::Pending);
    assert_eq!(recovered.payload(), &vec![1_u8, 2, 3]);
}

#[test]
fn reclaim_overlapped_drops_the_payload() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Marker;
    impl Drop for Marker {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    let overlapped = Operation::new(Marker).into_overlapped();
    // SAFETY: reclaim the leaked operation once, type-erased via its thunk.
    unsafe { super::reclaim_overlapped(overlapped) };
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
}
