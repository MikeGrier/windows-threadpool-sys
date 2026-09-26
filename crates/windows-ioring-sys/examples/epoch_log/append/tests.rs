// Copyright (c) 2026 Mike Grier
//! Tests for the append path's claim discipline.
//!
//! # Why these exist, and what they are the counterpart to
//!
//! `M22.2` found a real defect here: [`super::Appender::claim`] returned early
//! on a failed write without claiming the token, which `Token` deliberately
//! treats as still outstanding. The arena slot's outstanding count then never
//! returned to zero, `free_slots` never offered it again, and after `SLOTS`
//! such failures every append returned `WouldBlock` forever -- somewhere else
//! entirely, with no trace of the cause.
//!
//! The library has a test named for exactly that hazard,
//! `claiming_before_checking_the_result_is_what_stops_a_failure_from_leaking`
//! in `tests/failure_paths.rs`. This consumer had no counterpart, and the gap
//! was measured rather than suspected: reinstating the defect here -- moving
//! `completion.result()?` above the claim -- compiled and passed every test,
//! because nothing produced a failed write.
//!
//! A failed write is not rare enough to be unreachable, only rare enough to go
//! untested. [`Completion::with_injected_failure`] is the seam that reaches it
//! on demand, which is why `epoch_log` is a test target at all.

use std::os::windows::io::AsRawHandle;
use std::time::Duration;

#[cfg(feature = "fault-injection")]
use windows_ioring_sys::IoRingErrorExt;

use super::AppendRing;
use super::Appender;
// Used only by the fault-injection tests below, so the import is gated the
// same way they are -- an unconditional one warns on a default-feature build
// of the test target.
#[cfg(feature = "fault-injection")]
use super::SLOTS;
use crate::commit::Epoch;
use crate::placement::Placement;
use crate::record;

/// Hang bound on every wait here. Far above any real append.
const WAIT: Duration = Duration::from_secs(30);

/// The failure injected into an append's completion. Any error would do; the
/// point is that `result()` reports one.
#[cfg(feature = "fault-injection")]
const INJECTED: windows_ioring_sys::InjectedFailure =
    windows_ioring_sys::InjectedFailure::Win32(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED);

/// A scratch file to append to, named per test so tests running as threads in
/// one process cannot collide on it.
fn scratch(tag: &str) -> (std::path::PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-append-{}-{tag}.tmp",
        std::process::id()
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .expect("open fixture");
    (path, file)
}

/// **The contract's transfer requirement, enforced rather than merely stated
/// (M26.11).**
///
/// `epoch_log`'s contract requires a handle whose successful writes are
/// complete. The ring itself permits the opposite -- `RS-P-8` -- because it
/// never asks what kind of handle it was given, so this narrowing is the log's
/// own and has to be checked by the log.
///
/// **No real handle this sample opens will produce a short write**, which is
/// precisely why the check needs a seam to reach it: the branch would otherwise
/// be written once against the documentation and never executed again. That is
/// the same argument `M16.3` made for the failure seam, and
/// [`Completion::with_injected_transfer`] is its counterpart for a *successful*
/// short count, which a failure cannot model.
///
/// Both directions are asserted here, because a test that only shows the
/// rejection would pass just as well against a check that rejected everything.
/// The two cases differ in the injected count and in nothing else.
#[cfg(feature = "fault-injection")]
#[test]
fn a_short_write_violates_the_contracts_transfer_requirement() {
    let complete = record::RECORD_STRIDE;

    for (transferred, expect_accepted) in [(complete, true), (complete - 1, false)] {
        let mut ring = AppendRing::with_inventory(16, 16).expect("create ring");
        let (path, file) = scratch(&format!("short-write-{transferred}"));
        let mut appender =
            Appender::new(&mut ring, &Placement::decide(file.as_raw_handle())).expect("appender");

        let pushed = appender
            .append_batch(
                &mut ring,
                file.as_raw_handle(),
                Epoch(0),
                &[b"a record whose write will be reported short".to_vec()],
            )
            .expect("push one append");
        assert_eq!(pushed, 1, "a fresh arena always has a slot");

        let (completion, held) = ring
            .pop_within(WAIT)
            .expect("pop_within")
            .expect("the append's completion arrives well inside the bound");
        let (_payload, slot) = held.expect("an append carries its arena slot");
        let reported = completion.with_injected_transfer(transferred);

        match appender.claim(&reported, slot) {
            Ok(()) => {
                assert!(
                    expect_accepted,
                    "a write reporting {transferred} of {complete} bytes was accepted; the \
                     contract requires complete transfers and this one is short"
                );
            }
            Err(error) => {
                assert!(
                    !expect_accepted,
                    "a write reporting the full {complete} bytes was rejected: {error}"
                );
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::InvalidData,
                    "a handle that breaks a stated requirement is bad input, not an I/O failure"
                );
                let text = error.to_string();
                assert!(
                    text.contains(&transferred.to_string()) && text.contains(&complete.to_string()),
                    "the error must report both counts so the handle can be diagnosed, got: {text}"
                );
            }
        }

        drop(file);
        let _ = std::fs::remove_file(&path);
    }
}

/// **The regression guard `M22.2` earned and this consumer never had.**
///
/// A write that fails must still release its arena slot. The assertion is on
/// the slot rather than on the error, because the error was never the part that
/// broke: the old defect reported the failure correctly and leaked the slot
/// while doing it.
#[cfg(feature = "fault-injection")]
#[test]
fn a_failed_write_still_releases_its_arena_slot() {
    let mut ring = AppendRing::with_inventory(16, 16).expect("create ring");
    let (path, file) = scratch("failed-write");
    let mut appender =
        Appender::new(&mut ring, &Placement::decide(file.as_raw_handle())).expect("appender");

    let pushed = appender
        .append_batch(
            &mut ring,
            file.as_raw_handle(),
            Epoch(0),
            &[b"a record".to_vec()],
        )
        .expect("push one append");
    assert_eq!(pushed, 1, "the arena starts empty, so one record fits");
    assert_eq!(appender.in_flight(), 1);

    let (completion, held) = ring
        .pop_within(WAIT)
        .expect("pop_within")
        .expect("the append's completion arrives well inside the bound");
    let (_payload, slot) = held.expect("an append carries its arena slot");

    let failed = completion.with_injected_failure(INJECTED);
    let error = appender
        .claim(&failed, slot)
        .expect_err("a failed write must be reported, not swallowed");
    assert_eq!(
        (error.as_ioring_error().expect("an IoRingError").code() as u32) & 0xFFFF,
        windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED,
        "the error reaching the caller must be the one that was injected"
    );

    // The point of the test, and it has moved one layer down. The slot is
    // returned by the *pop*, not by anything `claim` does, so a failure path
    // that returned early could no longer leak it. What is still worth
    // asserting is that the appender accounts for the completion either way --
    // a failed write is still a completed operation. That the *arena* recovers
    // too is what `repeated_failures_never_exhaust_the_arena` below
    // establishes, by driving past `SLOTS` failures, which only completes if
    // slots are genuinely being handed back rather than merely appearing to be.
    assert_eq!(
        appender.in_flight(),
        0,
        "the completion must be accounted for even though the write failed"
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// The same discipline over enough failures to have exhausted the arena.
///
/// One leaked slot is survivable and invisible; `SLOTS` of them are what turned
/// `M22.2`'s defect into "every append returns `WouldBlock` forever". Driving
/// past that count is what distinguishes a slot that is released from one that
/// merely looks released once.
#[cfg(feature = "fault-injection")]
#[test]
fn repeated_failures_never_exhaust_the_arena() {
    let mut ring = AppendRing::with_inventory(16, 16).expect("create ring");
    let (path, file) = scratch("repeated-failures");
    let mut appender =
        Appender::new(&mut ring, &Placement::decide(file.as_raw_handle())).expect("appender");

    for round in 0..(SLOTS as usize * 2) {
        let pushed = appender
            .append_batch(
                &mut ring,
                file.as_raw_handle(),
                Epoch(0),
                &[b"a record".to_vec()],
            )
            .expect("push one append");
        assert_eq!(
            pushed, 1,
            "round {round}: a slot must be available, or an earlier failure leaked one"
        );

        let (completion, held) = ring
            .pop_within(WAIT)
            .expect("pop_within")
            .expect("the append's completion arrives well inside the bound");
        let (_payload, slot) = held.expect("an append carries its arena slot");
        appender
            .claim(&completion.with_injected_failure(INJECTED), slot)
            .expect_err("round {round}: the injected failure must be reported");
    }

    assert_eq!(appender.in_flight(), 0);
    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// A successful write releases its slot too, which is the control.
///
/// Without it the tests above would pass against an implementation that
/// released slots unconditionally at some later point, rather than because the
/// claim happened.
#[test]
fn a_successful_write_releases_its_arena_slot() {
    let mut ring = AppendRing::with_inventory(16, 16).expect("create ring");
    let (path, file) = scratch("successful-write");
    let mut appender =
        Appender::new(&mut ring, &Placement::decide(file.as_raw_handle())).expect("appender");

    appender
        .append_batch(
            &mut ring,
            file.as_raw_handle(),
            Epoch(0),
            &[b"a record".to_vec()],
        )
        .expect("push one append");

    let (completion, held) = ring
        .pop_within(WAIT)
        .expect("pop_within")
        .expect("the append's completion arrives well inside the bound");
    let (_payload, slot) = held.expect("an append carries its arena slot");
    appender
        .claim(&completion, slot)
        .expect("a successful claim");
    assert_eq!(appender.in_flight(), 0);

    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// A reused slot must not write the previous record's tail (M25.1).
///
/// Records are variable-length but land one per `RECORD_STRIDE` block, so the
/// write covers bytes the record itself never set. A slot is reused for the
/// log's whole life -- a `NumaBuffer` arrives zeroed, but only once -- so those
/// bytes are whatever the *previous*, longer record left in them.
///
/// **Replay cannot catch this, so the assertion is on the file's bytes.**
/// Replay decodes only at block starts and takes a record's extent from its own
/// header, so a stale fragment living past a short record's end is never read.
/// That makes leaving it a hygiene defect rather than a decode failure -- the
/// log would carry fragments of unrelated records, in a format whose whole
/// purpose is reconstructing what happened after a crash. Stating it that way
/// rather than as a corruption is deliberate: the zeroing is worth doing, and
/// claiming it prevents a decode error it cannot prevent would be worse than
/// not documenting it.
#[test]
fn a_reused_slot_does_not_write_the_previous_records_tail() {
    let mut ring = AppendRing::with_inventory(16, 16).expect("create ring");
    let (path, file) = scratch("reused-slot-tail");
    let mut appender =
        Appender::new(&mut ring, &Placement::decide(file.as_raw_handle())).expect("appender");

    // A long record, then a short one. `free_slots` hands out the lowest free
    // index, so draining between the two puts both records in slot 0 -- which
    // is what makes the second write's tail the first record's bytes.
    let long = vec![0xAB_u8; 200];
    let short = b"short".to_vec();

    for payload in [long, short.clone()] {
        let pushed = appender
            .append_batch(&mut ring, file.as_raw_handle(), Epoch(0), &[payload])
            .expect("push one append");
        assert_eq!(pushed, 1, "a drained arena always has a slot");
        let (completion, held) = ring
            .pop_within(WAIT)
            .expect("pop_within")
            .expect("the append's completion arrives well inside the bound");
        let (_payload, slot) = held.expect("an append carries its arena slot");
        appender
            .claim(&completion, slot)
            .expect("a successful claim");
    }

    let bytes = std::fs::read(&path).expect("read the log back");
    assert_eq!(
        bytes.len(),
        2 * record::RECORD_STRIDE,
        "two records occupy two whole blocks, whatever their own lengths"
    );

    let second = &bytes[record::RECORD_STRIDE..];
    let decoded = record::decode(second).expect("the short record decodes");
    assert_eq!(
        decoded.payload, short,
        "the second block holds the second record"
    );
    assert!(
        second[decoded.extent()..].iter().all(|&byte| byte == 0),
        "the rest of the block must be zero, not the 0xAB tail of the record \
         that used this slot before it"
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// Records land one per stride, and replay walks them back (M25.1 + M25.2).
///
/// **The guard this sample did not have.** M25.1 changed the writer's layout
/// and M25.2 the reader's, and between the two the log is unreadable -- replay
/// advances into a zeroed block tail and reports every record after the first
/// as missing. That was measured, not imagined: with the writer converted and
/// the reader not, every test in this file still passed, and only running the
/// example caught it. No CI job runs the example.
///
/// So this binds the two ends together at a rung that runs on every machine:
/// it appends through the real writer, reads the real file, and hands it to the
/// real reader. Either end changing alone turns it red.
#[test]
fn records_land_one_per_stride_and_replay_walks_them_back() {
    let payload_for = |index: usize| format!("record {index}: the quick brown fox").into_bytes();
    const COUNT: usize = 3;

    let mut ring = AppendRing::with_inventory(16, 16).expect("create ring");
    let (path, file) = scratch("stride-replay");
    let mut appender =
        Appender::new(&mut ring, &Placement::decide(file.as_raw_handle())).expect("appender");

    for index in 0..COUNT {
        let pushed = appender
            .append_batch(
                &mut ring,
                file.as_raw_handle(),
                Epoch(0),
                &[payload_for(index)],
            )
            .expect("push one append");
        assert_eq!(pushed, 1, "a drained arena always has a slot");
        let (completion, held) = ring
            .pop_within(WAIT)
            .expect("pop_within")
            .expect("the append's completion arrives well inside the bound");
        let (_payload, slot) = held.expect("an append carries its arena slot");
        appender
            .claim(&completion, slot)
            .expect("a successful claim");
    }

    let bytes = std::fs::read(&path).expect("read the log back");
    assert_eq!(
        bytes.len(),
        COUNT * record::RECORD_STRIDE,
        "each record occupies exactly one block"
    );

    let outcome = crate::replay::replay(&bytes, Epoch(0), COUNT, payload_for);
    assert!(
        outcome.is_clean(),
        "the reader must walk the writer's layout: {:?}",
        outcome.violations
    );
    assert_eq!(
        outcome.durable_verified, COUNT,
        "every record written must be read back, in sequence, with its payload intact"
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}
