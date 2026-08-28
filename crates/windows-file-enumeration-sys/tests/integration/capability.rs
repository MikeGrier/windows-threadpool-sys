// Copyright (c) 2026 Mike Grier
//! Unsupported-capability classification and the oversize-record path.
//!
//! # Why these are not exercised against a real incompatible filesystem
//!
//! `UnsupportedExtendedDirectoryInfo` and `RecordTooLarge` are both reached
//! only when a real `GetFileInformationByHandleEx` query fails with a
//! specific code -- `ERROR_INVALID_FUNCTION` / `ERROR_NOT_SUPPORTED` /
//! `ERROR_INVALID_PARAMETER` for the former, `ERROR_MORE_DATA` /
//! `ERROR_INSUFFICIENT_BUFFER` / `ERROR_BAD_LENGTH` for the latter. Producing
//! either organically needs a filesystem or redirector that genuinely refuses
//! `FileIdExtdDirectoryInfo`, and no such medium is available in this
//! environment (by design, `MINIMUM_BUFFER_CAPACITY` also guarantees the
//! latter is unreachable through the crate's own public buffer sizing on any
//! filesystem). This is a deliberate, acknowledged gap: both classifications
//! are proven exhaustively at the unit level in `native/tests.rs` against
//! synthetic Win32 codes (FE-11), which is the only way to reach them
//! deterministically without a filesystem this repository does not control.
//! If a real incompatible medium becomes available (a FAT-formatted removable
//! disk, a legacy SMB1 share, or similar), this file is where that coverage
//! belongs.

use windows_file_enumeration_sys::{EnumerationRequest, MINIMUM_BUFFER_CAPACITY};

use crate::support::Scratch;

#[test]
fn a_below_minimum_buffer_request_clamps_up_rather_than_reaching_an_oversize_record() {
    let scratch = Scratch::with_files(&["a.txt"]);
    let request = EnumerationRequest::for_path(scratch.path())
        .expect("resolvable")
        .with_buffer_capacity(1)
        .expect("representable");
    assert_eq!(
        request.buffer_capacity(),
        MINIMUM_BUFFER_CAPACITY,
        "a request below the minimum clamps up to it rather than ever reaching RecordTooLarge"
    );
}
