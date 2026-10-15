// Copyright (c) 2026 Mike Grier

use super::{lock_volume, unlock_volume};
use crate::test_support::{open_volume, ordinary_file, system_volume_path};

// A lock that succeeds is not tried here: it would hold a real volume against its other users for
// as long as the test ran, and needs an administrator. What these show is that the controls reach
// the device with no buffer and come back with the device's own answer, and that neither does
// anything to a volume that is not asked to be locked.

#[test]
fn unlocking_a_volume_this_handle_never_locked_is_refused_and_changes_nothing() {
    let volume = open_volume(&system_volume_path(), false);
    let refused = unlock_volume(&volume).expect_err("nothing is locked");
    assert!(
        refused.raw_os_error().is_some(),
        "the device's own error: {refused}"
    );
    // And again, on a handle that is overlapped, which the call waits on.
    let overlapped = open_volume(&system_volume_path(), true);
    assert!(unlock_volume(&overlapped).is_err());
}

#[test]
fn a_handle_that_is_not_a_volume_cannot_be_locked_or_unlocked() {
    let (file, path) = ordinary_file("lock");
    assert!(lock_volume(&file).is_err());
    assert!(unlock_volume(&file).is_err());
    drop(file);
    std::fs::remove_file(path).expect("remove");
}
