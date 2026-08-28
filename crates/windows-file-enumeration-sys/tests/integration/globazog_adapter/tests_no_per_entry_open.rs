// Copyright (c) 2026 Mike Grier
//! No path opens an individual entry.
//!
//! This is mostly a structural property, inherited from D-3 ("at most one
//! synchronous refill per worker callback", never a per-entry query) and
//! restated in [`adapter`](crate::globazog_adapter::adapter)'s own doc
//! comment. This file adds the one empirical check specific to this
//! adapter: a directory junction whose *target does not exist* is still
//! reported with full metadata. A backend that opened each entry
//! individually to resolve its metadata would have that open fail for this
//! one -- Windows resolves a reparse point when it is opened through, and
//! there is nothing at the other end of this one to resolve to -- so seeing
//! it delivered successfully, tag and all, is direct evidence the metadata
//! came from the batched directory listing alone.

use crate::globazog_adapter::adapter::enumerate_dir_native_via_wfe;
use crate::globazog_adapter::tests_support::ascii_name;
use crate::globazog_adapter::types::EnumPlan;
use crate::support::{Scratch, create_junction};

#[test]
fn a_junction_to_a_nonexistent_target_still_reports_full_metadata() {
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    let scratch = Scratch::empty();
    let link = scratch.child("dangling-junction");
    let missing_target = scratch.child("this-path-was-never-created");
    create_junction(&link, &missing_target);

    let scan = enumerate_dir_native_via_wfe(scratch.path(), EnumPlan::FULL)
        .expect("the directory listing itself never touches the junction's target");
    assert_eq!(scan.entries.len(), 1);
    let entry = &scan.entries[0];
    assert_eq!(ascii_name(entry), "dangling-junction");
    assert!(entry.is_reparse, "{entry:?}");
    assert_eq!(entry.reparse_tag, IO_REPARSE_TAG_MOUNT_POINT);
    assert_eq!(
        entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_REPARSE_POINT
    );

    // A control: confirm the target really is unreachable, so the assertion
    // above is not vacuously true because the target secretly does exist.
    let opened_through = std::fs::metadata(&link);
    assert!(
        opened_through.is_err(),
        "the fixture must actually be dangling for this test to prove anything: {opened_through:?}"
    );
}
