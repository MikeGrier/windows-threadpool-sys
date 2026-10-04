// Copyright (c) 2026 Mike Grier
//! Two contracts this crate states, which no other test asserted.
//!
//! Both were found while answering a question that has since been retired --
//! whether a topology realizer could build what it needs from this crate. That
//! question left, because the realizer is a component
//! [EP-D-5](../../topology-planner/DESIGN-NOTES.md#ep-d-5) places outside this
//! crate and which does not exist yet. These two assertions stayed, because
//! they are about this crate's own API and would be worth making if no planner
//! or realizer had ever been proposed.

#![cfg(windows)]

use win_numa_sys::{NumaBuffer, NumaNode};
use windows_ioring_sys::{IoBufMut, IoRing};

/// A ring's queue depths SATISFY what was asked for; they do not echo it.
///
/// An earlier version of this test asserted equality, and could not have caught
/// a kernel that ignored the request. Its three cases -- 8/16, 32/64, 512/1024
/// -- were already powers of two with the completion queue already twice the
/// submission queue, which is exactly the shape `CreateIoRing` normalises
/// towards, so every one came back untouched and the equality looked like a
/// contract. It was a property of the inputs.
///
/// Measured here by asking for shapes that are NOT already normalised: a
/// request of 9 submission entries comes back as 16, and 1 or 3 come back as 8.
/// The completion queue arrives at twice whatever the submission queue actually
/// became, not twice what was asked for -- so 9/9 yields 16/32, and a caller
/// who believed the old assertion would have sized a buffer against a number
/// the ring does not have.
///
/// What is asserted is therefore the relationship, not the values:
///
/// - the submission queue is a power of two and is **at least** what was asked
///   for, so a request is never silently under-served;
/// - the completion queue is at least what was asked for, and at least twice
///   the submission queue the ring actually built.
///
/// The discriminating cases live in the table below rather than in this
/// comment, because the table is executable and a comment is not. Removing the
/// non-power-of-two rows is what makes this test vacuous again.
#[test]
fn a_ring_satisfies_the_queue_depths_it_was_asked_for() {
    // Deliberately a mix: already-normalised shapes, which must pass through
    // unchanged, and shapes that cannot pass through unchanged. A table of only
    // the first kind is the defect this test was rewritten to remove.
    for (submission, completion) in [
        (8_u32, 16_u32),
        (32, 64),
        (512, 1024),
        (9, 9),
        (9, 17),
        (1, 1),
        (3, 5),
        (100, 100),
    ] {
        let ring = IoRing::new(submission, completion).expect("create ring");
        let info = ring.info().expect("the ring reports its info");

        assert!(
            info.submission_queue_size >= submission,
            "a ring must not under-serve the submission depth it was asked for: \
             asked {submission}, got {}",
            info.submission_queue_size
        );
        assert!(
            info.submission_queue_size.is_power_of_two(),
            "the submission queue is rounded to a power of two: asked {submission}, got {}",
            info.submission_queue_size
        );
        assert!(
            info.completion_queue_size >= completion,
            "a ring must not under-serve the completion depth it was asked for: \
             asked {completion}, got {}",
            info.completion_queue_size
        );
        assert!(
            info.completion_queue_size >= 2 * info.submission_queue_size,
            "the completion queue holds at least two entries per submission slot the ring \
             actually built: submission {}, completion {}",
            info.submission_queue_size,
            info.completion_queue_size
        );
    }
}

/// A node-bound allocation is usable as this crate's I/O memory.
///
/// `numa_buffer_io.rs` implements [`windows_ioring_sys::IoBuf`] and
/// [`IoBufMut`] for `NumaBuffer`, and nothing exercised them. The claim those
/// impls make is that placing a buffer on a memory node costs no new surface:
/// it is pushed and registered like a `Vec<u8>`.
///
/// The binding half is the generic function below -- if the impl were removed
/// this would fail to **compile**, which is a stronger rung than any assertion
/// for a claim about a trait being implemented.
#[test]
fn a_numa_buffer_is_this_crates_io_memory() {
    fn stable_mut_ptr_of<B: IoBufMut>(buffer: &mut B) -> *mut u8 {
        buffer.stable_mut_ptr()
    }

    // Node 0 exists everywhere, so this asserts the mechanism rather than the
    // machine.
    let mut buffer = NumaBuffer::new(4096, Some(NumaNode::new(0))).expect("the node allocates");

    assert_eq!(buffer.len(), 4096);
    assert!(!stable_mut_ptr_of(&mut buffer).is_null());
}
