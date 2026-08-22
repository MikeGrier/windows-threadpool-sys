// Copyright (c) 2026 Mike Grier
//! Unit tests for the coarse notification handle.

use super::CoarseHandle;
use crate::directory::OpenFailure;
use crate::testing::TempDir;
use crate::watcher::ALL_NOTIFY_FILTERS;

#[test]
fn opening_a_real_directory_succeeds() {
    let dir = TempDir::new("coarse-open");
    let handle = CoarseHandle::open(dir.path(), false, ALL_NOTIFY_FILTERS).expect("open");
    // SAFETY: the handle is consumed here and touched no further afterward.
    drop(unsafe { handle.into_waitable() });
    dir.cleanup();
}

#[test]
fn opening_a_path_that_does_not_exist_is_retryable() {
    let dir = TempDir::new("coarse-missing");
    let missing = dir.path().join("does-not-exist");
    let error = CoarseHandle::open(&missing, false, ALL_NOTIFY_FILTERS).expect_err("no such path");
    assert!(error.failure().is_retryable(), "{:?}", error.failure());
    assert_eq!(error.failure(), OpenFailure::NotFound);
    dir.cleanup();
}
