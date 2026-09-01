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

use core::ffi::c_void;

use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READ, PAGE_READWRITE};

use super::{
    CAPACITY, Ring, Slots, current_affinity, observed_node, pin_current_thread, working_set,
    working_set_flags,
};

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

#[test]
fn the_working_set_bit_layout_is_read_correctly() {
    // **The one assumption on this path that no other test can falsify.**
    // Every page on a single-node host is on node 0, so a wrong `NODE_SHIFT`
    // still reads zero and every other test here passes. The error would first
    // appear as nonsense node numbers on a multi-socket machine -- which is the
    // only machine whose answer this tool exists to collect, and the one place
    // nobody can check the result against anything.
    //
    // So this checks the offsets against a neighbouring field whose value the
    // caller chose. `Win32Protection` occupies the eleven bits immediately
    // before `Node`, and these pages were committed `PAGE_READWRITE`. Decoding
    // that correctly pins every offset up to where `Node` begins; getting it
    // wrong means the shift is wrong by exactly the amount that matters.
    let slots = Slots::on_node(CAPACITY, Some(0));
    let flags = working_set_flags(slots.ptr.cast()).expect("the page must be queryable");

    assert_ne!(
        flags & working_set::VALID,
        0,
        "a page that was just written is not marked resident, so the layout is wrong"
    );
    assert_eq!(
        (flags >> working_set::PROTECTION_SHIFT) & working_set::PROTECTION_MASK,
        PAGE_READWRITE as usize,
        "Win32Protection did not decode to what the allocation asked for, \
         so the field offsets -- including Node's -- are wrong: flags {flags:#x}"
    );
    assert_eq!(
        (flags >> working_set::SHARED_SHIFT) & 1,
        0,
        "a privately allocated page reported itself shared: flags {flags:#x}"
    );
}

#[test]
fn the_node_offset_is_pinned_by_a_page_whose_upper_fields_are_not_zero() {
    // The gap the private page above cannot close, found by sabotage: widening
    // `PROTECTION_BITS` from 11 to 12 moves `Node` by a bit, and a private
    // read-write page still decodes as `PAGE_READWRITE` because the bit swept
    // in -- `Shared` -- is zero there. Every field above the protection is zero
    // on a private page on a single-node host, and no arithmetic on zeros can
    // detect a shift.
    //
    // A code page is the counter-example, and it costs nothing to obtain: it is
    // mapped from the image, so it is shared, executable and read-only. Its
    // `Shared` bit is set, which means a wrong protection width swallows that
    // bit and decodes to a value that is not a protection constant at all.
    // Together with the private case this pins `Valid`, `ShareCount`,
    // `Win32Protection` and `Shared` -- and therefore where `Node` begins.
    let code_page =
        the_node_offset_is_pinned_by_a_page_whose_upper_fields_are_not_zero as *mut c_void;
    let flags = working_set_flags(code_page).expect("the running code must be queryable");

    assert_ne!(
        flags & working_set::VALID,
        0,
        "the page currently executing is not resident: flags {flags:#x}"
    );
    assert_eq!(
        (flags >> working_set::SHARED_SHIFT) & 1,
        1,
        "an image-backed code page did not report itself shared, so the bit \
         below Node is not where it is thought to be: flags {flags:#x}"
    );
    assert_eq!(
        (flags >> working_set::PROTECTION_SHIFT) & working_set::PROTECTION_MASK,
        PAGE_EXECUTE_READ as usize,
        "a code page's protection did not decode to PAGE_EXECUTE_READ, so the \
         protection field's width is wrong and Node's offset with it: flags {flags:#x}"
    );
}

#[test]
fn pinning_a_thread_restores_its_affinity_afterwards() {
    // **The leak this guards biased the measurements themselves.** The pin used
    // to discard the previous affinity, so a thread stayed confined after its
    // sample finished -- and the next sample allocated its ring while still on
    // the last sample's consumer, quietly placing memory that the report
    // describes as "left where it fell". The public timing helpers also left
    // their caller permanently re-affinitised.
    //
    // Run on a thread of this test's own, so a failure cannot disturb the rest
    // of the suite through the very leak it is checking for.
    std::thread::spawn(|| {
        let before = current_affinity().expect("the thread has an affinity");

        {
            let _pinned = pin_current_thread(Some((0, 0)));
            let during = current_affinity().expect("still has an affinity");
            assert_eq!(during.Mask, 1, "the pin did not take effect");
            assert_eq!(during.Group, 0);
        }

        let after = current_affinity().expect("the thread has an affinity");
        assert_eq!(
            (after.Mask, after.Group),
            (before.Mask, before.Group),
            "the affinity was not restored"
        );
    })
    .join()
    .expect("the pinning thread must not panic");
}

#[test]
fn asking_for_no_pin_leaves_the_affinity_alone() {
    std::thread::spawn(|| {
        let before = current_affinity().expect("the thread has an affinity");
        let guard = pin_current_thread(None);
        let during = current_affinity().expect("the thread has an affinity");
        assert_eq!((during.Mask, during.Group), (before.Mask, before.Group));
        drop(guard);
        let after = current_affinity().expect("the thread has an affinity");
        assert_eq!((after.Mask, after.Group), (before.Mask, before.Group));
    })
    .join()
    .expect("must not panic");
}

/// A processor number no group can hold, so `pin_current_thread` always fails.
///
/// 200 is past `usize::BITS`, which the pin asserts on before it ever reaches
/// Windows -- deterministic on every machine rather than dependent on which
/// processors happen to be online.
const UNPINNABLE: (u16, u8) = (0, 200);

/// Well inside the time either case takes when it works (both return in
/// milliseconds), and far outside the "never" the defect produced.
const MUST_FINISH_WITHIN: std::time::Duration = std::time::Duration::from_secs(20);

#[test]
fn a_failed_producer_pin_stops_the_run_rather_than_hanging_it() {
    // **The defect this guards.** `pin_current_thread` panics on failure. When
    // the producer was the one to fail, the consumer still entered `consume`
    // and spun forever on items no living thread would ever write -- an
    // unbounded loop with no deadline, so the process simply stopped making
    // progress. A run that should have failed loudly hung instead, which in CI
    // is a job timeout rather than a diagnosis.
    let started = std::time::Instant::now();
    let outcome = std::panic::catch_unwind(|| {
        super::time_model_on(super::Strategy::Baseline, Some(UNPINNABLE), None)
    });

    assert!(
        outcome.is_err(),
        "an impossible pin must not report success"
    );
    assert!(
        started.elapsed() < MUST_FINISH_WITHIN,
        "the run did not terminate: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_failed_consumer_pin_stops_the_run_rather_than_hanging_it() {
    // The other direction, and it hung for a different reason: the consumer
    // was pinned *after* the producer had been spawned, so the panic unwound
    // into `thread::scope`'s cleanup, which waits for a producer that is
    // itself blocked forever on a ring nobody is draining.
    let started = std::time::Instant::now();
    let outcome = std::panic::catch_unwind(|| {
        super::time_model_on(super::Strategy::Baseline, None, Some(UNPINNABLE))
    });

    assert!(
        outcome.is_err(),
        "an impossible pin must not report success"
    );
    assert!(
        started.elapsed() < MUST_FINISH_WITHIN,
        "the run did not terminate: {:?}",
        started.elapsed()
    );
}
