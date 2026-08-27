// Copyright (c) 2026 Mike Grier
//! Tests for the bounded completion ring.

use super::*;
use crate::error::{EnumerationError, Win32Error};
use crate::testing::named_file;
use std::os::windows::io::AsHandle;

fn ring(capacity: usize) -> Arc<CompletionRing> {
    Arc::new(CompletionRing::new(capacity))
}

fn entry(id: u64, name: &str) -> Completion {
    Completion::Entry {
        enumeration: EnumerationId::from_raw(id),
        entry: named_file(name),
    }
}

fn entry_name(record: &Completion) -> String {
    match record {
        Completion::Entry { entry, .. } => entry.name().to_string_lossy(),
        Completion::Terminal { .. } => panic!("expected an entry"),
    }
}

#[test]
fn an_empty_ring_accepts_entries_up_to_its_bound() {
    let ring = ring(4);
    for index in 0..4 {
        ring.try_send_entry(entry(1, &index.to_string()))
            .expect("within the bound");
    }
    assert_eq!(ring.len(), 4);
    assert!(!ring.has_data_room());
}

#[test]
fn a_full_ring_hands_the_entry_back_rather_than_dropping_it() {
    // The whole backpressure contract: a refused entry is still the caller's.
    let ring = ring(2);
    ring.try_send_entry(entry(1, "a")).expect("room");
    ring.try_send_entry(entry(1, "b")).expect("room");
    let refused = ring.try_send_entry(entry(1, "c")).expect_err("no room");
    assert_eq!(entry_name(&refused), "c");
    assert_eq!(ring.len(), 2);
}

#[test]
fn records_come_back_in_the_order_they_were_sent() {
    let ring = ring(4);
    for name in ["a", "b", "c"] {
        ring.try_send_entry(entry(1, name)).expect("room");
    }
    for name in ["a", "b", "c"] {
        let record = ring.try_take().expect("a queued record");
        assert_eq!(entry_name(&record), name);
    }
    assert!(ring.try_take().is_none());
}

#[test]
fn a_reservation_holds_room_that_entries_cannot_take() {
    let ring = ring(3);
    let slot = ring
        .reserve_terminal(EnumerationId::from_raw(1))
        .expect("room");
    // Capacity 3, one reserved, so only two entries fit.
    ring.try_send_entry(entry(1, "a")).expect("room");
    ring.try_send_entry(entry(1, "b")).expect("room");
    ring.try_send_entry(entry(1, "c")).expect_err("reserved");

    slot.send(TerminalOutcome::Completed);
    assert_eq!(ring.len(), 3);
}

#[test]
fn a_terminal_can_be_delivered_into_a_ring_that_is_otherwise_full() {
    // This is why the slot is claimed up front: the outcome must be reportable
    // exactly when there is no ordinary room left.
    let ring = ring(3);
    let slot = ring
        .reserve_terminal(EnumerationId::from_raw(7))
        .expect("room");
    ring.try_send_entry(entry(7, "a")).expect("room");
    ring.try_send_entry(entry(7, "b")).expect("room");
    assert!(!ring.has_data_room());

    slot.send(TerminalOutcome::Failed(EnumerationError::DirectoryQuery(
        Win32Error::from_code(5),
    )));

    // The two entries precede the terminal.
    assert_eq!(entry_name(&ring.try_take().expect("entry")), "a");
    assert_eq!(entry_name(&ring.try_take().expect("entry")), "b");
    let terminal = ring.try_take().expect("terminal");
    assert!(terminal.is_terminal());
    assert_eq!(terminal.enumeration(), EnumerationId::from_raw(7));
}

#[test]
fn reservations_never_consume_the_last_slot() {
    // Capacity 3 leaves room for two reservations; the third would leave no
    // slot an entry could ever use.
    let ring = ring(3);
    let _first = ring
        .reserve_terminal(EnumerationId::from_raw(1))
        .expect("room");
    let _second = ring
        .reserve_terminal(EnumerationId::from_raw(2))
        .expect("room");
    assert!(ring.reserve_terminal(EnumerationId::from_raw(3)).is_none());
}

#[test]
fn the_smallest_ring_carries_exactly_one_enumeration() {
    let ring = ring(MINIMUM_COMPLETION_CAPACITY);
    let slot = ring
        .reserve_terminal(EnumerationId::from_raw(1))
        .expect("room for one");
    assert!(ring.reserve_terminal(EnumerationId::from_raw(2)).is_none());
    assert!(ring.has_data_room());
    ring.try_send_entry(entry(1, "only")).expect("one entry");
    assert!(!ring.has_data_room());
    slot.send(TerminalOutcome::Completed);
    assert_eq!(ring.len(), 2);
}

#[test]
#[should_panic(expected = "one terminal and one entry")]
fn a_ring_of_one_is_rejected_at_construction() {
    let _ = CompletionRing::new(1);
}

#[test]
fn a_dropped_reservation_returns_its_slot() {
    let ring = ring(3);
    let slot = ring
        .reserve_terminal(EnumerationId::from_raw(1))
        .expect("room");
    ring.try_send_entry(entry(1, "a")).expect("room");
    ring.try_send_entry(entry(1, "b")).expect("room");
    assert!(!ring.has_data_room());

    // Receiver abandonment drops the slot without sending; the room comes back.
    drop(slot);
    assert!(ring.has_data_room());
    assert_eq!(ring.len(), 2);
}

#[test]
fn taking_a_record_frees_room_for_another() {
    let ring = ring(2);
    ring.try_send_entry(entry(1, "a")).expect("room");
    ring.try_send_entry(entry(1, "b")).expect("room");
    ring.try_send_entry(entry(1, "c")).expect_err("full");

    ring.try_take().expect("a record");
    ring.try_send_entry(entry(1, "c")).expect("room again");
}

#[test]
fn the_stream_ends_when_the_last_session_and_enumeration_are_gone() {
    let ring = ring(2);
    assert!(!ring.is_closed());
    ring.remove_session();
    assert!(ring.is_closed());
}

#[test]
fn an_outstanding_enumeration_keeps_the_stream_open() {
    // A session handle may be gone while an enumeration is still running, and
    // its terminal is still owed.
    let ring = ring(3);
    let slot = ring
        .reserve_terminal(EnumerationId::from_raw(1))
        .expect("room");
    ring.remove_session();
    assert!(!ring.is_closed());
    slot.send(TerminalOutcome::Completed);
    assert!(ring.is_closed());
}

#[test]
fn extra_session_handles_each_hold_the_stream_open() {
    let ring = ring(2);
    ring.add_session();
    ring.remove_session();
    assert!(!ring.is_closed());
    ring.remove_session();
    assert!(ring.is_closed());
}

#[test]
fn a_blocking_take_returns_none_once_the_stream_ends() {
    let ring = ring(2);
    ring.remove_session();
    assert!(ring.take_blocking(None).is_none());
}

#[test]
fn a_blocking_take_drains_what_is_queued_before_reporting_the_end() {
    let ring = ring(2);
    ring.try_send_entry(entry(1, "a")).expect("room");
    ring.remove_session();
    assert!(ring.take_blocking(None).is_some());
    assert!(ring.take_blocking(None).is_none());
}

#[test]
fn a_timed_take_gives_up_without_ending_the_stream() {
    let ring = ring(2);
    assert!(ring.take_blocking(Some(Duration::from_millis(1))).is_none());
    assert!(!ring.is_closed());
}

#[test]
fn a_waiting_receiver_is_woken_by_a_send() {
    let ring = ring(4);
    let producer = Arc::clone(&ring);
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        producer.try_send_entry(entry(1, "late")).expect("room");
    });
    let record = ring
        .take_blocking(Some(Duration::from_secs(5)))
        .expect("the send wakes the wait");
    assert_eq!(entry_name(&record), "late");
    handle.join().expect("producer");
}

#[test]
fn a_waiting_receiver_is_woken_by_the_end_of_the_stream() {
    let ring = ring(4);
    let producer = Arc::clone(&ring);
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        producer.remove_session();
    });
    assert!(ring.take_blocking(Some(Duration::from_secs(5))).is_none());
    handle.join().expect("producer");
}

#[test]
fn the_doorbell_starts_signalled_when_records_already_wait() {
    // Created under the ring lock, so its initial state cannot disagree with a
    // ring that already has something to observe.
    let ring = ring(2);
    ring.try_send_entry(entry(1, "a")).expect("room");
    let handle = ring.doorbell().expect("an event");
    assert!(is_signalled(handle));
}

#[test]
fn the_doorbell_tracks_exactly_what_the_receiver_can_observe() {
    let ring = ring(3);
    let handle = ring.doorbell().expect("an event");
    assert!(!is_signalled(handle), "an idle ring has nothing to observe");

    ring.try_send_entry(entry(1, "a")).expect("room");
    assert!(is_signalled(handle));

    ring.try_take().expect("a record");
    assert!(!is_signalled(handle), "the ring is empty again");
}

#[test]
fn the_doorbell_stays_signalled_once_the_stream_ends() {
    // A waiter must learn about the end of the stream, not wait for a record
    // that can never arrive.
    let ring = ring(2);
    let handle = ring.doorbell().expect("an event");
    assert!(!is_signalled(handle));
    ring.remove_session();
    assert!(is_signalled(handle));
}

#[test]
fn a_terminal_delivery_signals_the_doorbell() {
    let ring = ring(3);
    let slot = ring
        .reserve_terminal(EnumerationId::from_raw(1))
        .expect("room");
    let handle = ring.doorbell().expect("an event");
    assert!(!is_signalled(handle));
    slot.send(TerminalOutcome::Cancelled);
    assert!(is_signalled(handle));
}

#[test]
fn an_owned_doorbell_refers_to_the_same_event() {
    let ring = ring(2);
    let owned = ring.doorbell_owned().expect("a duplicate");
    ring.try_send_entry(entry(1, "a")).expect("room");
    assert!(is_signalled(owned.as_handle()));
}

/// Whether a waitable handle is currently signalled.
fn is_signalled(handle: BorrowedHandle<'_>) -> bool {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    // SAFETY: the handle is a live event owned by the ring under test.
    let result = unsafe { WaitForSingleObject(handle.as_raw_handle() as HANDLE, 0) };
    result == WAIT_OBJECT_0
}
