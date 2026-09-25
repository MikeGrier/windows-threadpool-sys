// Copyright (c) 2026 Mike Grier
//! Tests for the epoch bookkeeping (M21.4).
//!
//! The case these exist for cannot be reached by *running* the sample: a
//! commit only fails if its flush fails, and a flush against a healthy temp
//! file does not. The crate's fault-injection seam
//! ([`Completion::with_injected_failure`]) is what makes the failed-commit
//! path reachable at all, which is the same reason `tests/fault_injection.rs`
//! exists one layer down.
//!
//! Only the tests that *need* the seam are gated on `fault-injection`, so a
//! default `cargo test` still runs the rest. The gated ones are covered by
//! CI's `cargo test --workspace --all-features` job, the same job that covers
//! `tests/fault_injection.rs`; a local run needs `--features fault-injection`
//! to see them.

use std::os::windows::io::AsRawHandle;
use std::time::Duration;

use windows_ioring_sys::{Completion, IoRing};

#[cfg(feature = "fault-injection")]
use windows_ioring_sys::IoRingErrorExt;

use super::{Committer, Epoch};

/// Hang bound on every wait here. Far above any real flush.
const WAIT: Duration = Duration::from_secs(30);

/// The failure injected into a commit's completion. Any error would do; the
/// point is that `result()` reports one.
#[cfg(feature = "fault-injection")]
const INJECTED: windows_ioring_sys::InjectedFailure =
    windows_ioring_sys::InjectedFailure::Win32(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED);

/// A scratch file to commit against, named per test so tests running as
/// threads in one process cannot collide on it.
fn scratch(tag: &str) -> (std::path::PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-commit-{}-{tag}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"x").expect("create fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open fixture");
    (path, file)
}

/// Close the open epoch and wait for its commit's completion, returning both.
///
/// Uses the bounded pop published in M21.2, which is what makes this a wait
/// rather than the spin these tests would otherwise have had to write.
fn commit_and_pop(
    ring: &mut IoRing,
    committer: &mut Committer,
    file: &std::fs::File,
) -> (Epoch, Completion) {
    let closed = committer
        .commit(ring, file.as_raw_handle())
        .expect("push the commit");
    let completion = ring
        .pop_within(WAIT)
        .expect("pop_within")
        .expect("the commit's completion arrives well inside the bound");
    (closed, completion)
}

#[cfg(feature = "fault-injection")]
#[test]
fn a_failed_commit_leaves_its_epoch_not_durable() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (path, file) = scratch("failed");
    let mut committer = Committer::new();

    let (closed, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    let error = committer
        .claim(&completion.with_injected_failure(INJECTED))
        .expect_err("a failed flush must be reported, not swallowed");
    // The crate preserves the HRESULT rather than classifying it, so the
    // kind is `Other` and the Win32 code is what identifies the failure --
    // the same assertion `tests/fault_injection.rs` makes one layer down.
    assert_eq!(
        (error.as_ioring_error().expect("an IoRingError").code() as u32) & 0xFFFF,
        windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED,
        "the error reaching the caller must be the one that was injected"
    );

    assert!(
        committer.durable_through().is_none(),
        "a failed commit advances the watermark not at all"
    );
    assert!(
        !committer.is_durable(closed),
        "the epoch its flush was named for is not durable yet"
    );
    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "fault-injection")]
#[test]
fn a_later_successful_commit_covers_an_epoch_whose_own_commit_failed() {
    // The claim this item exists to bind. Epoch 0's flush fails; epoch 1's
    // flush is covering, so it reaches epoch 0's writes too, and observing it
    // makes epoch 0 durable after all.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (path, file) = scratch("covered");
    let mut committer = Committer::new();

    let (first, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    committer
        .claim(&completion.with_injected_failure(INJECTED))
        .expect_err("epoch 0's own commit fails");
    // Asserted before the second commit, so this test cannot pass against an
    // implementation that reported `true` all along.
    assert!(
        !committer.is_durable(first),
        "epoch 0 must not be durable between its failure and the next success"
    );

    let (second, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    let advanced = committer
        .claim(&completion)
        .expect("epoch 1's commit succeeds")
        .expect("and it is one of ours");
    assert_eq!(advanced, second);

    assert!(
        committer.is_durable(first),
        "epoch 1's covering flush reached epoch 0's writes, so epoch 0 is durable now"
    );
    assert!(committer.is_durable(second));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_epoch_above_the_watermark_is_never_durable() {
    // The other direction of the guard above: a `is_durable` that answered
    // `true` unconditionally would pass every assertion in this file except
    // this one.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (path, file) = scratch("above");
    let mut committer = Committer::new();

    let (closed, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    committer
        .claim(&completion)
        .expect("the commit succeeds")
        .expect("and it is ours");

    assert!(committer.is_durable(closed));
    assert!(
        !committer.is_durable(committer.open_epoch()),
        "the epoch still open was never committed and must not report durable"
    );
    assert!(
        !committer.is_durable(Epoch(closed.0 + 7)),
        "nor may an epoch that does not exist yet"
    );
    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "fault-injection")]
#[test]
fn durability_stays_monotonic_across_a_failed_commit() {
    // Three epochs, the middle one's commit failing. Once the third settles,
    // every epoch at or below the watermark must report durable -- which is
    // the property that lets a caller remember one number instead of a set.
    let mut ring = IoRing::new(16, 32).expect("create ring");
    let (path, file) = scratch("monotonic");
    let mut committer = Committer::new();

    let (first, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    committer.claim(&completion).expect("epoch 0 commits");

    let (second, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    committer
        .claim(&completion.with_injected_failure(INJECTED))
        .expect_err("epoch 1's commit fails");
    assert!(
        !committer.is_durable(second),
        "the watermark must not move past a failure"
    );
    assert!(
        committer.is_durable(first),
        "and it must not move backwards either"
    );

    let (third, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    committer.claim(&completion).expect("epoch 2 commits");

    for epoch in 0..=third.0 {
        assert!(
            committer.is_durable(Epoch(epoch)),
            "epoch {epoch} is at or below the watermark and must report durable"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "fault-injection")]
#[test]
fn a_failed_commit_is_no_longer_in_flight() {
    // The epoch is removed from the in-flight map *before* the result is
    // checked. Were it removed after, a failed commit would be accounted for
    // forever and the log could never quiesce.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (path, file) = scratch("in-flight");
    let mut committer = Committer::new();

    let (_, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    assert_eq!(committer.in_flight(), 1, "pushed but not yet observed");
    committer
        .claim(&completion.with_injected_failure(INJECTED))
        .expect_err("the commit fails");
    assert_eq!(
        committer.in_flight(),
        0,
        "a failure still settles the accounting; only the watermark is withheld"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_completion_that_belongs_to_someone_else_is_not_claimed() {
    // A drain loop hands every completion to every claimant, so answering
    // "not mine" without touching the watermark is load-bearing rather than
    // defensive. The foreign completion here is real rather than fabricated:
    // a flush pushed directly, bypassing the committer entirely.
    use windows_ioring_sys::{Batch, FlushCoverage, FlushMode};

    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (path, file) = scratch("foreign");
    let mut committer = Committer::new();

    let (closed, completion) = commit_and_pop(&mut ring, &mut committer, &file);
    committer.claim(&completion).expect("our own commit");
    let before = committer.durable_through();

    let mut batch = Batch::new(&mut ring);
    // SAFETY: `file` outlives this operation -- it is drained below, before
    // the test returns.
    let foreign_id = unsafe {
        batch.flush_raw(
            file.as_raw_handle(),
            FlushCoverage::Unordered,
            FlushMode::Default,
        )
    }
    .expect("queue a flush nobody is tracking");
    batch.submit().expect("submit");
    let foreign = ring
        .pop_within(WAIT)
        .expect("pop_within")
        .expect("the foreign flush completes");
    assert_eq!(foreign.user_data(), foreign_id);

    assert!(
        committer
            .claim(&foreign)
            .expect("a foreign completion is not an error")
            .is_none(),
        "a completion this committer never pushed is not its business"
    );
    assert_eq!(
        committer.durable_through(),
        before,
        "and it must not move the watermark"
    );
    assert!(committer.is_durable(closed));
    let _ = std::fs::remove_file(&path);
}
