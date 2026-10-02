// Copyright (c) 2026 Mike Grier
//! Realizing a plan-shaped description against a synthetic machine (`M27.2`).
//!
//! [REALIZATION-CENSUS.md](../REALIZATION-CENSUS.md) walked what a topology
//! realizer would need from this crate and marked four of five plan facts
//! **covered**. Those verdicts were reached by reading signatures, which is the
//! weakest kind of evidence this repository accepts for a claim about its own
//! code. This file converts them into a demonstration: a realizer, written
//! against nothing but the public API, that builds the arrangement a plan
//! describes.
//!
//! # The plan type here is a stand-in, and binds nothing
//!
//! `EP-1+.5` and `EP-1+.6` have not settled what a plan is, and `M27.2`'s
//! gap-closing half is gated on that. So [`DomainPlan`] below is **local to
//! this test**: it carries the four facts `M27.1` enumerated and nothing else,
//! it is not exported, and no public surface is proposed or changed. When the
//! real vocabulary lands, this is a thing to delete and rewrite, not a
//! commitment to honour.
//!
//! # Synthetic means the plan is not read off this machine
//!
//! The thesis asks that this crate be realizable without the hardware a plan
//! describes, so the plan is written as a literal rather than discovered by
//! querying the host. One concession is unavoidable and is called out at its
//! assertion: a *memory node* must exist for an allocation to land, so the
//! plan names node 0, which every machine has. That makes the allocation real
//! rather than mocked; it does not make the plan machine-derived.

#![cfg(windows)]

use win_numa_sys::{NumaBuffer, NumaNode};
use windows_ioring_sys::{IoRing, RingVersion};

/// The four facts `M27.1` says a plan states about one domain.
///
/// A stand-in. See the module docs: nothing public binds to this.
struct DomainPlan {
    /// Which processor the domain pins to.
    ///
    /// Carried because the plan states it, **not** because it can be
    /// realized -- see [`Unrealized`].
    pin_to_processor: u32,
    /// Which memory node the domain's pool allocates from.
    memory_node: Option<NumaNode>,
    /// How many queues, and how deep.
    queues: Vec<QueuePlan>,
    /// Where each channel's buffer lives, as a length to allocate from
    /// `memory_node`.
    channel_buffer_lens: Vec<usize>,
}

struct QueuePlan {
    submission_depth: u32,
    completion_depth: u32,
}

/// A plan fact the realizer could not express through this crate's public API.
///
/// # This records a gap; it does not detect one
///
/// Absence of an API cannot be asserted in Rust, so this is a statement in test
/// code rather than a measurement of the crate. Its value is that it sits in
/// the same file as the realizer: somebody adding pool placement has to come
/// here to stop the realizer reporting it, which is the moment the census needs
/// updating. It will not notice on its own.
#[derive(Debug, PartialEq, Eq)]
enum Unrealized {
    ProcessorPinning { requested: u32 },
}

/// What realizing one domain produced.
struct Realized {
    rings: Vec<IoRing>,
    buffers: Vec<NumaBuffer>,
    unrealized: Vec<Unrealized>,
}

/// Build the arrangement `plan` describes, using only this crate's public API.
fn realize(plan: &DomainPlan) -> std::io::Result<Realized> {
    // Fact 1: the processor a domain pins to.
    //
    // There is no expression for this. An `IoRing` has no thread of its own,
    // and the pool its completions would be delivered on exposes thread
    // *counts* but not thread *placement*. Recorded rather than silently
    // skipped, so the realizer's output says what it could not honour.
    //
    // A `vec![]` of one rather than a push, at clippy's suggestion; it grows
    // again the moment a second fact turns out to be unrealizable.
    let unrealized = vec![Unrealized::ProcessorPinning {
        requested: plan.pin_to_processor,
    }];

    // Fact 3: how many queues, of which type, and how deep.
    let mut rings = Vec::new();
    for queue in &plan.queues {
        rings.push(IoRing::with_version(
            RingVersion::V1,
            queue.submission_depth,
            queue.completion_depth,
        )?);
    }

    // Facts 2 and 4: which memory node, and where each channel's buffer lives.
    // The same mechanism answers both -- a buffer's address is chosen by
    // whoever allocates it, and this crate accepts any `IoBuf`.
    let mut buffers = Vec::new();
    for len in &plan.channel_buffer_lens {
        buffers.push(NumaBuffer::new(*len, plan.memory_node)?);
    }

    Ok(Realized {
        rings,
        buffers,
        unrealized,
    })
}

fn a_plan() -> DomainPlan {
    DomainPlan {
        pin_to_processor: 3,
        // Node 0 exists everywhere; see the module docs for why this one fact
        // is not a free literal.
        memory_node: Some(NumaNode::new(0)),
        queues: vec![
            QueuePlan {
                submission_depth: 32,
                completion_depth: 64,
            },
            QueuePlan {
                submission_depth: 8,
                completion_depth: 16,
            },
        ],
        channel_buffer_lens: vec![4096, 8192],
    }
}

#[test]
fn a_plan_is_realizable_from_the_public_api_alone() {
    let plan = a_plan();
    let realized = realize(&plan).expect("the plan realizes");

    assert_eq!(
        realized.rings.len(),
        plan.queues.len(),
        "a realizer builds one ring per queue the plan names"
    );
    assert_eq!(
        realized.buffers.len(),
        plan.channel_buffer_lens.len(),
        "a realizer builds one buffer per channel the plan names"
    );
}

#[test]
fn the_queue_depths_a_plan_names_reach_the_ring() {
    // The census marks depth "covered" because `IoRing::new` takes both sizes.
    // Asserting the ring *reports back* what the plan asked for is what makes
    // that more than a claim about a signature: a constructor that accepted the
    // numbers and ignored them would pass the signature reading and fail here.
    let plan = a_plan();
    let realized = realize(&plan).expect("the plan realizes");

    // Asserted before the loop, because a `zip` over an empty `rings` iterates
    // zero times and would make every assertion below vacuously true.
    assert_eq!(
        realized.rings.len(),
        plan.queues.len(),
        "nothing below means anything if no ring was built"
    );
    assert!(!plan.queues.is_empty(), "the plan must name a queue");

    for (ring, queue) in realized.rings.iter().zip(&plan.queues) {
        let info = ring.info().expect("the ring reports its info");
        // Equality, not `>=`. Measured on this host: a ring reports back
        // exactly the depths it was asked for, at 8/16, 32/64 and 512/1024. A
        // `>=` would pass a constructor that ignored both parameters and always
        // built something larger, which is the failure worth excluding.
        assert_eq!(
            info.submission_queue_size, queue.submission_depth,
            "the ring must carry the submission depth the plan named"
        );
        assert_eq!(
            info.completion_queue_size, queue.completion_depth,
            "the ring must carry the completion depth the plan named"
        );
    }
}

#[test]
fn a_channel_buffer_lands_where_the_plan_put_it_and_is_usable_as_io_memory() {
    // The census's load-bearing claim about memory placement: `NumaBuffer` is
    // not merely re-exported, it *satisfies this crate's buffer traits*, so a
    // node-bound allocation needs no new surface to be used as I/O memory.
    //
    // Deliberately not gated on IoRing support: this asserts the buffer half,
    // which is true whether or not the host can create a ring.
    let plan = a_plan();
    let mut buffer =
        NumaBuffer::new(plan.channel_buffer_lens[0], plan.memory_node).expect("the node allocates");

    assert_eq!(buffer.len(), plan.channel_buffer_lens[0]);

    // The whole point: it is usable as this crate's I/O memory. If `IoBuf` were
    // not implemented for it, this would not compile -- which is the strongest
    // rung available for this particular claim.
    fn accepts_io_memory<B: windows_ioring_sys::IoBufMut>(b: &mut B) -> *mut u8 {
        b.stable_mut_ptr()
    }
    assert!(!accepts_io_memory(&mut buffer).is_null());
}

#[test]
fn the_realizer_reports_exactly_the_gap_the_census_names() {
    // The bidirectional half. The census says *one* plan fact is unrealizable;
    // this pins the count, so a realizer that quietly stopped honouring
    // something else would be caught here rather than in prose.
    //
    // What it cannot do is notice a gap *closing* -- see `Unrealized`.
    let plan = a_plan();
    let realized = realize(&plan).expect("the plan realizes");

    assert_eq!(
        realized.unrealized,
        vec![Unrealized::ProcessorPinning { requested: 3 }],
        "the census names processor pinning as the only unrealizable plan fact; \
         if this changed, REALIZATION-CENSUS.md is now wrong"
    );
}
