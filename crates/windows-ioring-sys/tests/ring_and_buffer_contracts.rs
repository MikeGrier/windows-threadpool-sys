// Copyright (c) 2026 Mike Grier
//! Two contracts this crate states and nothing else asserted.
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

/// A ring carries back exactly the queue depths it was asked for.
///
/// The nearest existing assertion is `info.submission_queue_size > 0`, which a
/// constructor that ignored both arguments would satisfy. Equality is available
/// -- measured at 8/16, 32/64 and 512/1024 on this host, each returned
/// unchanged -- so it is what gets asserted.
#[test]
fn a_ring_reports_back_the_queue_depths_it_was_asked_for() {
    for (submission, completion) in [(8_u32, 16_u32), (32, 64), (512, 1024)] {
        let ring = IoRing::new(submission, completion).expect("create ring");
        let info = ring.info().expect("the ring reports its info");

        assert_eq!(
            info.submission_queue_size, submission,
            "a ring must carry the submission depth it was created with"
        );
        assert_eq!(
            info.completion_queue_size, completion,
            "a ring must carry the completion depth it was created with"
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
