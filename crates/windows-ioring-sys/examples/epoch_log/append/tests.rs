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

use windows_ioring_sys::IoRing;
#[cfg(feature = "fault-injection")]
use windows_ioring_sys::IoRingErrorExt;

use super::{Appender, SLOTS};
use crate::commit::Epoch;
use crate::placement::Placement;

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

/// **The regression guard `M22.2` earned and this consumer never had.**
///
/// A write that fails must still release its arena slot. The assertion is on
/// the slot rather than on the error, because the error was never the part that
/// broke: the old defect reported the failure correctly and leaked the slot
/// while doing it.
#[cfg(feature = "fault-injection")]
#[test]
fn a_failed_write_still_releases_its_arena_slot() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
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

    let completion = ring
        .pop_within(WAIT)
        .expect("pop_within")
        .expect("the append's completion arrives well inside the bound");

    let failed = completion.with_injected_failure(INJECTED);
    let error = appender
        .claim(&failed)
        .expect_err("a failed write must be reported, not swallowed");
    assert_eq!(
        (error.as_ioring_error().expect("an IoRingError").code() as u32) & 0xFFFF,
        windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED,
        "the error reaching the caller must be the one that was injected"
    );

    // The point of the test. Claiming is what returns the slot; a claim skipped
    // on the failure path would leave this at one and the leak would be
    // invisible until the arena ran dry. That the *arena* recovers too is what
    // `repeated_failures_never_exhaust_the_arena` below establishes, by driving
    // past `SLOTS` failures -- which only completes if slots are genuinely
    // being handed back rather than merely appearing to be.
    assert_eq!(
        appender.in_flight(),
        0,
        "the token must have been claimed even though the write failed"
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
    let mut ring = IoRing::new(16, 16).expect("create ring");
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

        let completion = ring
            .pop_within(WAIT)
            .expect("pop_within")
            .expect("the append's completion arrives well inside the bound");
        appender
            .claim(&completion.with_injected_failure(INJECTED))
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
    let mut ring = IoRing::new(16, 16).expect("create ring");
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

    let completion = ring
        .pop_within(WAIT)
        .expect("pop_within")
        .expect("the append's completion arrives well inside the bound");
    assert!(appender.claim(&completion).expect("a successful claim"));
    assert_eq!(appender.in_flight(), 0);

    drop(file);
    let _ = std::fs::remove_file(&path);
}
