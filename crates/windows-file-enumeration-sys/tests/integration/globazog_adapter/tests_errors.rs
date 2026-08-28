// Copyright (c) 2026 Mike Grier
//! The error-plus-partial-listing surface: a root that cannot be enumerated
//! at all reproduces Globazog's own "no usable listing" contract, and
//! `finish_scan`'s translation of a late, partial failure is proven directly
//! -- see [`globazog_adapter`](crate::globazog_adapter)'s module doc comment
//! for why a live mid-stream failure cannot be manufactured here.

use windows_file_enumeration_sys::{EnumerationError, TerminalOutcome, Win32Error};

use crate::globazog_adapter::adapter::{enumerate_dir_native_via_wfe, finish_scan};
use crate::globazog_adapter::tests_support::ascii_name;
use crate::globazog_adapter::types::EnumPlan;
use crate::support::Scratch;

#[test]
fn a_missing_root_is_a_fatal_error_with_no_dirscan_produced() {
    let scratch = Scratch::empty();
    let error = enumerate_dir_native_via_wfe(&scratch.child("does-not-exist"), EnumPlan::FULL)
        .expect_err("a missing root yields no usable listing");
    // Any `io::Error` is acceptable here; the property under test is that it
    // is `Err`, not `Ok(DirScan)`, matching win.rs's own "no usable listing"
    // contract for an unopenable root.
    let _ = error;
}

#[test]
fn finish_scan_reports_a_fatal_error_when_failure_leaves_no_entries() {
    let outcome = TerminalOutcome::Failed(EnumerationError::DirectoryQuery(Win32Error::from_code(
        5, // ERROR_ACCESS_DENIED; the specific code is not the point here.
    )));
    let result = finish_scan(Vec::new(), outcome);
    assert!(
        result.is_err(),
        "a failure with no entries collected must be a fatal error, not an empty DirScan"
    );
}

#[test]
fn finish_scan_preserves_a_partial_listing_and_reports_one_entry_error() {
    let scratch = Scratch::with_files(&["already-delivered.txt"]);
    // Run a real, successful scan to obtain one genuine translated `DirEntry`
    // -- not a hand-built stand-in -- and then feed it to `finish_scan`
    // alongside a synthetic `Failed` outcome, which is exactly the
    // situation a real late failure would leave `finish_scan` to translate.
    let ok = enumerate_dir_native_via_wfe(scratch.path(), EnumPlan::FULL).expect("a scan");
    assert_eq!(ok.entries.len(), 1);

    let outcome =
        TerminalOutcome::Failed(EnumerationError::DirectoryQuery(Win32Error::from_code(5)));
    let scan = finish_scan(ok.entries, outcome).expect(
        "a failure with entries already collected must truncate the listing, not discard it",
    );
    assert_eq!(scan.entries.len(), 1);
    assert_eq!(ascii_name(&scan.entries[0]), "already-delivered.txt");
    assert_eq!(scan.entry_errors.len(), 1);
    assert!(
        scan.entry_errors[0].name.is_none(),
        "this backend never attributes a late failure to one specific entry"
    );
    let message = scan.entry_errors[0].source.to_string();
    assert!(
        !message.is_empty(),
        "the underlying OS error must still be reachable through EntryFailure::source"
    );
}
