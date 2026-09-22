// Copyright (c) 2026 Mike Grier
//! Tests for the measurement harness's lane (M22.2).
//!
//! These exist because collapsing the sample's two free-slot definitions into
//! [`crate::append::free_slots`] did not only remove a duplicate -- it removed
//! a slot leak the duplicate had and nothing reported. A tracked free list
//! took its slot *before* composing into it, so an append refused between the
//! two -- a record too long for a slot is the reachable one -- returned with
//! the slot removed from the list and no operation ever issued.
//!
//! That leak was verified by re-injecting the free list, not argued from the
//! source: eight refused appends left the lane reporting **0** of 8 slots free
//! while the arena held nothing.
//!
//! # What this file does and does not catch
//!
//! [`a_record_too_long_for_a_slot_leaves_its_slot_free`] asserts the property
//! from the arena's side, so it would catch an append path that marks a slot
//! busy before the record is known to fit. It would **not** catch a
//! re-introduced free list, because it never asks one -- under the re-injection
//! above it passed, and only a temporary assertion against the list itself went
//! red. What keeps a second definition from coming back is that there is one
//! function and both callers call it, which the compiler enforces and a grep
//! can confirm; this file cannot, and saying otherwise would be the sort of
//! cosmetic binding these tests exist to avoid.
//!
//! A `Lane` owns a real ring, so by this crate's Quality rule these are not
//! hermetic. They are example-target tests rather than lib tests, so they do
//! not add to the pile [D-49](../../../DESIGN-NOTES.md#d-49) queues for `M24`;
//! and the ring is incidental here, because nothing below ever submits.

use std::os::windows::io::AsRawHandle;

use super::{Lane, SLOT_LEN, SLOTS};
use crate::append::free_slots;
use crate::commit::Epoch;
use crate::record::HEADER_LEN;

/// A scratch file to append against, named per test so tests running as
/// threads in one process cannot collide on it.
fn scratch(tag: &str) -> (std::path::PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-strategy-{}-{tag}.tmp",
        std::process::id()
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .expect("a scratch file in the temp directory");
    (path, file)
}

/// How many of this lane's slots are free, asked the same way the append path
/// asks it.
fn free(lane: &Lane) -> usize {
    free_slots(&lane.arena, SLOTS as usize).len()
}

/// A payload one byte too long to fit a slot once the header is accounted for.
///
/// Derived from the two constants rather than written as a number, so a change
/// to either cannot leave this test passing for the wrong reason.
fn oversized() -> Vec<u8> {
    vec![0xAB; SLOT_LEN - HEADER_LEN + 1]
}

#[test]
fn a_fresh_lane_has_every_slot_free() {
    let lane = Lane::new(None).expect("a ring and a registered arena");
    assert_eq!(
        free(&lane),
        SLOTS as usize,
        "a lane that has pushed nothing must offer every slot"
    );
}

#[test]
fn a_record_too_long_for_a_slot_is_refused() {
    let mut lane = Lane::new(None).expect("a ring and a registered arena");
    let (path, file) = scratch("too-long-refused");

    let error = lane
        .append_batch(file.as_raw_handle(), 0, Epoch(0), &oversized(), 0, 1)
        .expect_err("a record that cannot fit a slot must not be accepted");

    assert_eq!(
        error.kind(),
        std::io::ErrorKind::InvalidInput,
        "the refusal is the encoder's, and it reports a bad input rather than a busy arena"
    );

    drop(file);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_record_too_long_for_a_slot_leaves_its_slot_free() {
    let mut lane = Lane::new(None).expect("a ring and a registered arena");
    let (path, file) = scratch("too-long-no-leak");
    assert_eq!(free(&lane), SLOTS as usize, "precondition: nothing is busy");

    // Fail an append `SLOTS` times over. Every one is refused before any
    // operation is pushed, so the arena is untouched by all of them.
    for _ in 0..SLOTS {
        lane.append_batch(file.as_raw_handle(), 0, Epoch(0), &oversized(), 0, 1)
            .expect_err("a record that cannot fit a slot must not be accepted");
    }

    // The regression this test exists for. A tracked free list took its slot
    // *before* composing into it, so each failure above consumed one and never
    // gave it back: this lane would report zero free slots while the kernel
    // held nothing, and every later append would be told to drain an arena
    // that was entirely idle. Deriving the answer from the arena's own
    // outstanding counts cannot express that -- a slot nothing was pushed
    // against never stopped being free.
    assert_eq!(
        free(&lane),
        SLOTS as usize,
        "a refused append must not consume a slot"
    );

    drop(file);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_lane_offers_at_most_what_was_asked_for() {
    let lane = Lane::new(None).expect("a ring and a registered arena");
    assert_eq!(free_slots(&lane.arena, 3).len(), 3, "capped by the request");
    assert_eq!(
        free_slots(&lane.arena, 0).len(),
        0,
        "asking for none is answerable without consulting the arena"
    );
    assert_eq!(
        free_slots(&lane.arena, SLOTS as usize * 4).len(),
        SLOTS as usize,
        "and capped by the arena when the request exceeds it"
    );
}
