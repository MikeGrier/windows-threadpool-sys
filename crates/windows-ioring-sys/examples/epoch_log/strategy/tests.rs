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

use super::{CommitStrategy, Lane, SLOT_LEN, SLOTS};
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

/// A commit's cost is reported in three parts that add up (M25.4).
///
/// The harness published one blended number until `M20.6` decomposed it by
/// hand and found it was **entirely deferral** -- a figure that looked like a
/// commit latency and measured how long the next epoch's appends took. The
/// split is the fix, and this pins the two properties that make it a fix
/// rather than a rename:
///
/// - the parts **sum** to the old number, so nothing was lost in splitting;
/// - `flush()` **excludes** deferral, which is the whole point.
///
/// It deliberately asserts nothing about the *values*. Which part carries the
/// cost is a property of the machine and the handle, not of this crate, and
/// `M25`'s standing constraint forbids depending on an operation pending.
#[test]
fn a_commits_cost_is_reported_in_parts_that_do_not_overlap() {
    let (path, file) = scratch("timing-parts");
    let payload = b"a harness record".to_vec();

    let outcome = super::run(
        CommitStrategy::CoveringFlush,
        file.as_raw_handle(),
        3,
        4,
        &payload,
        None,
    )
    .expect("a small run completes");

    assert!(
        !outcome.commit_timings.is_empty(),
        "a run of three epochs must time three commits"
    );
    for timing in &outcome.commit_timings {
        assert_eq!(
            timing.flush(),
            timing.submit + timing.blocking,
            "the flush's own cost is its submit plus its wait, and nothing else"
        );
        assert!(
            timing.flush() <= timing.submit + timing.blocking + timing.deferral,
            "deferral must not be folded into the flush: that is the defect M20.6 found"
        );
    }

    // The identity above is satisfied by a part that is never measured at all,
    // which is not hypothetical: replacing the deferral measurement with zero
    // was **survived** by the assertions above alone. A part that always reads
    // zero is a column of zeros in the report and a decomposition in name only.
    //
    // Asserted as "some sample is non-zero" rather than a lower bound on any
    // duration. This harness defers by construction -- it pushes the next
    // epoch's appends before settling the previous commit -- so a run in which
    // *nothing* deferred means the clock is not running, not that the machine
    // was fast. `blocking` gets no such assertion, because zero is a legitimate
    // and frequently observed reading for it.
    assert!(
        outcome
            .commit_timings
            .iter()
            .any(|timing| !timing.deferral.is_zero()),
        "a harness that defers by design must observe some deferral, or it is not measuring it"
    );
    assert!(
        outcome
            .commit_timings
            .iter()
            .any(|timing| !timing.submit.is_zero()),
        "submitting a flush must cost something, or it is not being measured"
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// All three strategies must write byte-identical logs (M25.1b).
///
/// This is `compare_strategies`' strongest assertion and the one with real
/// teeth: replay checks a log against itself, where this checks the three
/// against *each other*, so a dropped record, a wrong offset, or an ordering
/// bug in any one of them shows up as a difference from the other two.
///
/// It had no test. The harness test above runs `CoveringFlush` alone, and
/// `main`'s version needs all three -- which made it the one assertion in the
/// sample that only `cargo run` could reach. Running them at two epochs of
/// three records instead of thirty-two of sixty-four puts it at a rung that
/// runs everywhere, for a cost the suite does not notice.
///
/// **Why identical bytes is the right expectation even across ring counts:**
/// a record's offset is its position in the global sequence times the stride,
/// and a strategy decides which *ring* submits a write, never where it lands.
/// `AlternatingRings` therefore interleaves submissions across two lanes into
/// the same offset space and must still produce the same file.
#[test]
fn every_strategy_writes_the_same_log() {
    const EPOCHS: usize = 2;
    const PER_EPOCH: usize = 3;
    let payload = b"a harness record".to_vec();

    let mut reference: Option<(&'static str, Vec<u8>)> = None;
    for strategy in super::CommitStrategy::ALL {
        let (path, file) = scratch(&format!("cross-{}", strategy.name()));
        let outcome = super::run(
            strategy,
            file.as_raw_handle(),
            EPOCHS,
            PER_EPOCH,
            &payload,
            None,
        )
        .expect("a small run completes");
        assert_eq!(
            outcome.records,
            EPOCHS * PER_EPOCH,
            "{} did not write every record it was asked for",
            strategy.name()
        );

        let bytes = std::fs::read(&path).expect("read the log back");
        drop(file);
        let _ = std::fs::remove_file(&path);

        match &reference {
            None => reference = Some((strategy.name(), bytes)),
            Some((first, expected)) => assert_eq!(
                &bytes,
                expected,
                "{} produced a different log than {first}; all three must write the same bytes",
                strategy.name()
            ),
        }
    }
}
///
/// **The gap this closes was measured, not suspected.** The harness's `Lane`
/// is a second writer over the same on-disk format as the log's own
/// `Appender`, and when the stride was introduced nothing tested it: reverting
/// this lane to a packed layout left every test in the example green. The only
/// thing that exercised it was running the sample, and no CI job does.
///
/// `CoveringFlush` specifically, because it uses **one** ring. The multi-ring
/// strategies interleave two lanes into one offset space, so the on-disk order
/// is not the sequence order and replay's in-order check would refuse a
/// perfectly good log. That is a property of the harness rather than of the
/// format, so this test picks the configuration where the format's own rule is
/// the only thing under test.
#[test]
fn a_run_lays_its_records_out_one_per_stride() {
    let (path, file) = scratch("stride-layout");
    let payload = b"a harness record".to_vec();

    let outcome = super::run(
        CommitStrategy::CoveringFlush,
        file.as_raw_handle(),
        2,
        3,
        &payload,
        None,
    )
    .expect("a small run completes");

    let bytes = std::fs::read(&path).expect("read the log back");
    assert_eq!(
        bytes.len(),
        outcome.records * crate::record::RECORD_STRIDE,
        "every record must occupy exactly one block"
    );

    let replayed = crate::replay::replay(&bytes, outcome.durable_through, outcome.records, |_| {
        payload.clone()
    });
    assert!(
        replayed.is_clean(),
        "the harness writes the same format the reader walks: {:?}",
        replayed.violations
    );
    assert_eq!(replayed.durable_verified, outcome.records);

    drop(file);
    let _ = std::fs::remove_file(&path);
}
