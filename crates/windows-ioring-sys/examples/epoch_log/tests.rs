// Copyright (c) 2026 Mike Grier
//! The sample's own end-to-end verification, run by `cargo test` (M25.1b).
//!
//! # Why this exists
//!
//! [`crate::replay`] calls itself "the only part that can catch a durability
//! bug, because it is the only part that checks the claim [`crate::contract`]
//! actually makes rather than the steps taken to reach it". Until this file,
//! **nothing ran it except a human typing `cargo run`.** No CI job executes
//! the example, and `cargo test --example epoch_log` compiles `main.rs` as a
//! test harness without ever calling `main`.
//!
//! That was not a theoretical gap. During `M25.1` the writer was converted to a
//! strided layout and the reader was not, which left the log unreadable -- and
//! **every one of the example's tests passed anyway**. Only running the sample
//! caught it.
//!
//! # Why a test rather than a CI step
//!
//! `M25.1b` framed this as a choice between the two. A CI job running the
//! binary is the obvious answer, and it is the wrong rung: it catches a defect
//! after it is pushed, on a machine the author is not sitting at. The
//! repository's FAIL FAST rule asks for the lowest rung that can carry the
//! fact, and this one carries it -- `run_log` and `verify` are ordinary
//! functions over a generic [`crate::Report`], so a test can drive the real log
//! against a real ring and a real file and then run the real verifier, which is
//! the same code path `main` takes.
//!
//! This is not a proxy for running the sample. It *is* running the sample,
//! minus the part below.
//!
//! # What is deliberately left out
//!
//! [`crate::strategy`]'s three-way comparison, which `main` runs after
//! `verify`. It is the expensive part -- thirty-two epochs of sixty-four
//! records, three times -- and its layout and replay are covered by
//! `strategy::tests::a_run_lays_its_records_out_one_per_stride`, which was
//! added by `M25.1` for the same reason this file exists. Running it again here
//! would multiply the suite's cost to re-check something already checked.
//!
//! So `cargo run --example epoch_log` remains the only thing that exercises the
//! comparison end to end, and that is a deliberate line rather than an
//! oversight: what it would add is a measurement, and a measurement is not a
//! contract this crate can assert.

use std::path::PathBuf;

use crate::{Report, run_log, verify};

/// Scratch paths for this test, named so they cannot collide with the
/// process-scoped paths `main` uses or with another test running as a sibling
/// thread.
fn scratch(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-selftest-{}-{tag}",
        std::process::id()
    ))
}

/// The whole sample, end to end: append, commit, checkpoint, reclaim, then all
/// three replay passes and the negative control.
///
/// The assertions live inside [`verify`] -- that a clean log reports no
/// violations, that a torn tail is tolerated rather than rejected, and that
/// corrupting a byte inside the durable region **is** reported. The last is
/// what makes the other two mean anything, and it is why this test does not
/// add assertions of its own: duplicating them here would be a second copy of
/// the contract check, which is the defect `M25.1` spent its time removing.
#[test]
fn the_sample_keeps_its_contract_and_its_verifier_can_still_fail() {
    // Output goes to buffers rather than stdout: a passing test should be
    // silent, and `Report` is generic over its writers precisely so a caller
    // can decide where the narrative lands.
    let mut report = Report::new(Vec::new(), Vec::new());

    let log = scratch("log");
    let retired = scratch("retired");
    let checkpoint = scratch("checkpoint");

    let outcome = run_log(&mut report, &log, &retired, &checkpoint)
        .and_then(|run| verify(&mut report, &log, &run));

    // Cleaned up before the assertion, so a failure does not also leave files
    // behind for the next run to trip over.
    for path in [&log, &retired, &checkpoint] {
        let _ = std::fs::remove_file(path);
    }

    outcome.expect("the sample must run and verify its own contract");
}
