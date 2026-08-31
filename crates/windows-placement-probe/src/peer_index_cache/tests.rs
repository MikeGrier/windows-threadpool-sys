// Copyright (c) Mike Grier.

//! Tests for the ring's memory placement.
//!
//! The timing itself is not testable offline -- it needs two real cores and
//! several seconds -- but the *honesty* of the placement is, and that is the
//! part a reader of the output depends on. A row claiming the ring sat on node
//! 3 when it did not is worse than a row admitting it does not know, because
//! nothing downstream can tell the difference.
//!
//! Every case here runs on a single-node host, which is what every machine
//! available to this workspace is. Node 0 exists everywhere Windows runs, and a
//! node that cannot exist is the other half of the pair.

use super::{CAPACITY, Ring, first_processor_of_node};

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
        ring.memory_node, None,
        "a ring that was never placed claimed a node"
    );
}

#[test]
fn a_ring_placed_on_an_existing_node_records_it() {
    // Node 0 exists on every Windows host, including the single-node VM slices
    // this is developed on, so this case is reachable without NUMA hardware.
    let ring = Ring::new_on(CAPACITY, Some(0));

    assert_eq!(
        ring.memory_node,
        Some(0),
        "a ring placed on node 0 did not record it"
    );
}

#[test]
fn a_ring_that_could_not_be_placed_records_none_rather_than_the_node_it_wanted() {
    // The defect this exists to prevent. The obvious implementation stores the
    // requested node and never revisits it, so a failed placement produces a
    // row that reads exactly like a successful one. `memory_node` must be what
    // the run *achieved*, not what it asked for.
    let ring = Ring::new_on(CAPACITY, Some(ABSENT_NODE));

    assert_eq!(
        ring.memory_node, None,
        "a ring recorded a node it could not be placed on"
    );
}

#[test]
fn the_first_processor_of_an_absent_node_is_none() {
    assert_eq!(
        first_processor_of_node(ABSENT_NODE),
        None,
        "a node that cannot exist offered a processor"
    );
}

#[test]
fn node_zero_offers_a_processor_to_touch_from() {
    // If this ever fails, placement silently degrades to "unknown" on every row
    // rather than erroring, so it is worth asserting the happy path exists at
    // all rather than inferring it from the ring test above.
    assert!(
        first_processor_of_node(0).is_some(),
        "node 0 offered no processor, so nothing can place memory on it"
    );
}

#[test]
fn a_placed_ring_is_still_a_usable_ring() {
    // Placement writes to every slot from another thread. That must leave the
    // ring in its initial state and not, say, a half-full one -- a ring whose
    // indices moved would time a shorter run and report it as a faster one.
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
