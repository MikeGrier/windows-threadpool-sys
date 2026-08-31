// Copyright (c) Mike Grier.

//! Tests for the ring's memory placement.
//!
//! The timing itself is not testable offline -- it needs two real cores and
//! several seconds -- but the *honesty* of the placement is, and that is the
//! part a reader of the output depends on. A row claiming the ring sat on node
//! 3 when it did not is worse than a row admitting it does not know, because
//! nothing downstream can tell the difference.
//!
//! **These replace tests that passed against a mechanism that never worked.**
//! The first implementation placed memory by first touch and asserted only that
//! the bookkeeping field held the requested node -- which it did, unconditionally,
//! because the pages had already been faulted in by the vector that allocated
//! them. Asserting the field agrees with the request cannot detect that; these
//! ask the operating system where the pages are instead.
//!
//! Every case here runs on a single-node host, which is what every machine
//! available to this workspace is. Node 0 exists everywhere Windows runs, and a
//! node that cannot exist is the other half of the pair.

use super::{CAPACITY, Ring, Slots, observed_node};

/// A node id no machine will have.
///
/// `u32::MAX` rather than a plausible-but-large number: the point is to be
/// certainly absent, not probably absent, so the test cannot start passing for
/// the wrong reason on a big enough machine.
const ABSENT_NODE: u32 = u32::MAX;

#[test]
fn a_ring_asked_for_no_placement_records_none() {
    let ring = Ring::new_on(CAPACITY, None);

    assert_eq!(
        ring.memory_node(),
        None,
        "a ring that was never placed claimed a node"
    );
}

#[test]
fn a_ring_placed_on_an_existing_node_records_it() {
    // Node 0 exists on every Windows host, including the single-node VM slices
    // this is developed on, so this case is reachable without NUMA hardware.
    let ring = Ring::new_on(CAPACITY, Some(0));

    assert_eq!(
        ring.memory_node(),
        Some(0),
        "a ring placed on node 0 did not record it"
    );
}

#[test]
fn a_ring_never_records_a_node_it_could_not_be_placed_on() {
    // The defect this exists to prevent, and the one that actually shipped:
    // storing the requested node and never revisiting it produces a row that
    // reads exactly like a successful placement.
    //
    // Asserting `None` here would be wrong, and finding that out is why this
    // test earns its place. `VirtualAllocExNuma` is documented to reject an
    // out-of-range node, and measured on this host it does not -- asking for
    // `u32::MAX` returns pages on node 0. The ring genuinely is on node 0, so
    // reporting node 0 is the truth and reporting `None` would discard it. The
    // property that must hold is narrower and is the one that matters: the
    // record never names the node that was merely *asked for*.
    let ring = Ring::new_on(CAPACITY, Some(ABSENT_NODE));

    assert_ne!(
        ring.memory_node(),
        Some(ABSENT_NODE),
        "a ring recorded a node it could not be placed on"
    );
}

#[test]
fn the_recorded_node_comes_from_the_pages_and_not_from_the_request() {
    // The distinction the previous implementation could not make. Ask the
    // operating system directly about the same allocation, and require the
    // recorded value to be what it says -- so a `Slots` that stored its
    // argument would fail here even when the argument happened to be right.
    let slots = Slots::on_node(CAPACITY, Some(0));

    let from_the_pages = observed_node(slots.ptr.cast());

    assert_eq!(
        slots.node, from_the_pages,
        "the recorded node is not what the pages report"
    );
}

#[test]
fn an_impossible_request_still_yields_a_working_ring() {
    // Whichever way the allocation goes -- honoured, quietly redirected, or
    // refused into the heap fallback -- the storage must be usable and its
    // node must be either unknown or genuinely observed. A ring that quietly
    // lost its slots would fail far away from here.
    let slots = Slots::on_node(CAPACITY, Some(ABSENT_NODE));

    assert_eq!(slots.len(), CAPACITY, "the request lost the slots");
    assert_eq!(
        slots.node,
        observed_node(slots.ptr.cast()),
        "the recorded node disagrees with the pages"
    );
}

#[test]
fn an_unplaced_page_reports_no_node_rather_than_node_zero() {
    // `observed_node` reads a bitfield in which an absent answer and node 0
    // are both all-zero bits, distinguished only by the `Valid` flag. Reading
    // it wrong would report every unplaced page as node 0 -- a wrong answer
    // that looks entirely reasonable. A null address is never resident.
    assert_eq!(
        observed_node(core::ptr::null_mut()),
        None,
        "an unqueryable address reported a node"
    );
}

#[test]
fn a_placed_ring_is_still_a_usable_ring() {
    // Placement writes to every slot. That must leave the ring in its initial
    // state and not, say, a half-full one -- a ring whose indices moved would
    // time a shorter run and report it as a faster one.
    let ring = Ring::new_on(CAPACITY, Some(0));

    assert_eq!(ring.slots.len(), CAPACITY, "placement resized the ring");
    assert_eq!(
        ring.head.0.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "placement advanced the head"
    );
    assert_eq!(
        ring.tail.0.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "placement advanced the tail"
    );
}

#[test]
fn every_slot_of_every_path_is_readable_and_zero() {
    // **Both paths, and that is the whole point of this test.** An earlier
    // version checked only the placed ring, and the heap ring shipped a bug it
    // could not see: its elements were built as `i32` by type inference while
    // the pointer read them as `u64`, so the second half of every heap ring was
    // memory belonging to something else. Reading it was undefined behaviour
    // and writing it corrupted the heap -- and a check of `len()` alone, which
    // is what the suite had, reports 1024 either way.
    //
    // `None` is the path the unpinned and by-placement measurements take, so it
    // is the one that runs most often, not an edge case.
    for node in [None, Some(0)] {
        let ring = Ring::new_on(CAPACITY, node);

        for (index, slot) in ring.slots.iter().enumerate() {
            // SAFETY: no other thread exists in this test, so nothing else
            // holds a reference to any slot.
            let value = unsafe { *slot.get() };
            assert_eq!(value, 0, "slot {index} of {node:?} was not initialised");
        }
    }
}

#[test]
fn dropping_a_placed_ring_releases_its_pages() {
    // The NUMA path frees with `VirtualFree` and the heap path by rebuilding a
    // `Box`; getting the pair the wrong way round corrupts the heap or leaks a
    // region. Repeated allocation makes a leak of whole pages visible as a
    // failure to allocate rather than as slow growth nobody notices.
    for _ in 0..256 {
        let ring = Ring::new_on(CAPACITY, Some(0));
        assert_eq!(ring.slots.len(), CAPACITY);
        drop(ring);
    }
    for _ in 0..256 {
        drop(Ring::new_on(CAPACITY, None));
    }
}
