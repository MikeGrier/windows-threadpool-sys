// Copyright (c) 2026 Mike Grier
//! That the fault-injection seam is reachable, and gated, from outside the
//! crate (M16.3).
//!
//! M16.4 uses the seam to cover the failure paths that have no coverage at
//! all. This file checks the thing M16.4's tests cannot: that the seam is
//! actually exported under the feature, and actually absent without it.
//!
//! The whole file compiles to nothing unless `fault-injection` is on, so a
//! default `cargo test` skips it silently -- which would ordinarily mean tests
//! that exist without ever executing. CI's `cargo test --workspace
//! --all-features` job is what stops that, since `--all-features` enables this
//! one; verified rather than assumed. A local run needs
//! `--features fault-injection` to see anything here at all.

#![cfg(all(windows, feature = "fault-injection"))]

use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;

use windows_ioring_sys::{
    Batch, Completion, InjectedFailure, IoRing, IoRingErrorExt, PushOptions, RingCondition,
};

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-fault-injection-{tag}-{}.tmp",
        std::process::id()
    ))
}

/// Push a real read, wait for it, and hand back the token and its genuine
/// completion.
fn real_read(
    ring: &mut IoRing,
    path: &std::path::Path,
) -> (windows_ioring_sys::Token<Vec<u8>>, Completion) {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .expect("open the fixture");

    let mut batch = Batch::new(ring);
    // SAFETY: `file` outlives the operation, and the token is returned to the
    // caller, which holds it until the completion is claimed.
    let token =
        unsafe { batch.read_raw(file.as_raw_handle(), vec![0_u8; 5], 0, PushOptions::new()) }
            .expect("queue a read");
    batch.submit_and_wait(1, 30_000).expect("submit and wait");
    let completion = loop {
        if let Some(completion) = ring.try_pop().expect("pop") {
            break completion;
        }
    };
    (token, completion)
}

#[test]
fn the_seam_is_reachable_from_an_integration_test() {
    // The reason this file exists. The seam's whole purpose is letting tests
    // outside the crate exercise failure handling, so being reachable only
    // from `#[cfg(test)]` unit tests would leave it useless for the job.
    let path = temp_file("reachable");
    std::fs::write(&path, b"hello").expect("create the fixture");

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let (token, completion) = real_read(&mut ring, &path);
    completion
        .result()
        .expect("the read really did succeed, or this test proves nothing");

    let injected = completion.with_injected_failure(InjectedFailure::Ring(RingCondition::Corrupt));
    let error = injected.result().expect_err("the injected failure applies");
    assert_eq!(error.ring_condition(), Some(RingCondition::Corrupt));

    // And the token still claims it, because the operation genuinely finished.
    // This is the property that makes the seam safe rather than a
    // use-after-free vector, exercised from where a consumer would exercise it.
    let buffer = token
        .claim_if(&injected)
        .expect("a failed completion still claims its own token");
    assert_eq!(buffer, b"hello");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_win32_code_survives_the_hresult_wrapping() {
    // A consumer matching on a Win32 error needs the injected one to read back
    // exactly as a kernel-produced one would -- `(hresult as u32) & 0xFFFF` is
    // how this crate's own tests recover a real one.
    let path = temp_file("win32");
    std::fs::write(&path, b"hello").expect("create the fixture");

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let (token, completion) = real_read(&mut ring, &path);

    let injected = completion.with_injected_failure(InjectedFailure::Win32(
        windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED,
    ));
    let error = injected.result().expect_err("the injected failure applies");
    assert_eq!(
        (error.as_ioring_error().expect("an IoRingError").code() as u32) & 0xFFFF,
        windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED
    );

    let _ = token
        .claim_if(&injected)
        .expect("claims its own completion");
    let _ = std::fs::remove_file(&path);
}
