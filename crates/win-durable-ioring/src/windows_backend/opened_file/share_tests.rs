// Copyright (c) 2026 Mike Grier
//! What the share mode asks of the kernel, and what the kernel does with two opens of one file
//! (DI-13.4, DI-D-64). The two opens are real: each combination of access and sharing on each side is
//! made and judged against what the `CreateFileW` documentation says of the share modes.

use std::any::Any;
use std::io;
use std::path::Path;

use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

use super::super::{FileRights, NoData, ReadWrite, ShareMode};
use super::{Cached, FileOptions, OpenedBy, OpenedFile, SharingViolation, data_access};
use crate::dioring::tests::{TempFile, wtf};
use durable_ioring::Access;

/// What an open can ask for that a share mode is judged against: one of the three things the share
/// mode names. An open that asks for none of them is not judged at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wants {
    Read,
    Write,
    Delete,
}

pub(super) const WANTS: [Wants; 3] = [Wants::Read, Wants::Write, Wants::Delete];

/// Every subset of the three members, the empty one first.
pub(super) fn every_share_mode() -> Vec<ShareMode> {
    (0..8)
        .map(|mask| {
            [ShareMode::READ, ShareMode::WRITE, ShareMode::DELETE]
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .fold(ShareMode::NONE, |set, (_, member)| set | *member)
        })
        .collect()
}

/// The member of the share mode that allows another open to ask for `wants`.
pub(super) fn shares(wants: Wants) -> ShareMode {
    match wants {
        Wants::Read => ShareMode::READ,
        Wants::Write => ShareMode::WRITE,
        Wants::Delete => ShareMode::DELETE,
    }
}

/// Open `path` asking for `wants` and sharing `share`; the handle is boxed so that two of different
/// types can be held at once.
fn open(path: &Path, wants: Wants, share: ShareMode) -> io::Result<Box<dyn Any>> {
    Ok(match wants {
        Wants::Read => Box::new(OpenedFile::<Cached, _>::open(
            wtf(path),
            FileOptions::new().read().share(share),
        )?),
        Wants::Write => Box::new(OpenedFile::<Cached, _>::open(
            wtf(path),
            FileOptions::new().write().share(share),
        )?),
        Wants::Delete => Box::new(OpenedFile::<Cached, NoData>::open(
            wtf(path),
            FileOptions::new().rights(FileRights::DELETE).share(share),
        )?),
    })
}

/// The `CreateFileW` documentation's rule for two opens of one file, one from each side: the later
/// open fails when it asks for something the earlier open does not share, and when the earlier open
/// asked for something the later one does not share.
pub(super) fn documented_to_conflict(
    first: (Wants, ShareMode),
    second: (Wants, ShareMode),
) -> bool {
    !first.1.contains(shares(second.0)) || !second.1.contains(shares(first.0))
}

pub(super) fn sharing_violation(error: &io::Error) -> &SharingViolation {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<SharingViolation>())
        .expect("the cause is a SharingViolation")
}

/// The Win32 code of the kernel's error, which a violation keeps as its source.
pub(super) fn kernel_code(violation: &SharingViolation) -> Option<i32> {
    std::error::Error::source(violation)
        .and_then(|source| source.downcast_ref::<io::Error>())
        .and_then(io::Error::raw_os_error)
}

#[test]
fn every_combination_of_two_opens_is_accepted_or_refused_as_the_documentation_says() {
    let temp = TempFile::new("share-matrix");
    let modes = every_share_mode();
    assert_eq!(modes.len(), 8, "the empty set and the seven others");
    let mut conflicts = 0;
    for first_wants in WANTS {
        for &first_share in &modes {
            let first = open(&temp.0, first_wants, first_share).unwrap_or_else(|e| {
                panic!("the first open of {first_wants:?}/{first_share:?}: {e}")
            });
            for second_wants in WANTS {
                for &second_share in &modes {
                    let outcome = open(&temp.0, second_wants, second_share);
                    let conflict = documented_to_conflict(
                        (first_wants, first_share),
                        (second_wants, second_share),
                    );
                    assert_eq!(
                        outcome.is_err(),
                        conflict,
                        "{first_wants:?} sharing {first_share:?}, then {second_wants:?} sharing \
                         {second_share:?}"
                    );
                    if let Err(error) = outcome {
                        conflicts += 1;
                        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
                        assert_eq!(
                            kernel_code(sharing_violation(&error)),
                            Some(ERROR_SHARING_VIOLATION.cast_signed())
                        );
                    }
                }
            }
            drop(first);
        }
    }
    assert!(
        conflicts > 0,
        "the matrix reached refusals as well as acceptances"
    );
}

#[test]
fn a_few_combinations_by_name_so_the_matrix_is_not_the_only_statement() {
    use Wants::{Delete, Read, Write};
    let both = ShareMode::READ | ShareMode::WRITE;
    // (first open, second open, whether the second is refused)
    let cases = [
        // A reader sharing read admits another reader, and refuses a writer and a deleter.
        ((Read, ShareMode::READ), (Read, ShareMode::READ), false),
        ((Read, ShareMode::READ), (Write, ShareMode::ALL), true),
        ((Read, ShareMode::READ), (Delete, ShareMode::ALL), true),
        // A writer sharing everything still refuses a reader that does not share its write.
        ((Write, ShareMode::ALL), (Read, ShareMode::READ), true),
        ((Write, ShareMode::ALL), (Read, both), false),
        // Sharing delete is what admits a deleter, and the deleter must share what is held.
        ((Read, ShareMode::ALL), (Delete, ShareMode::READ), false),
        ((Read, ShareMode::ALL), (Delete, ShareMode::NONE), true),
        (
            (Delete, ShareMode::DELETE),
            (Delete, ShareMode::DELETE),
            false,
        ),
        ((Delete, ShareMode::READ), (Read, ShareMode::DELETE), false),
        // An open that shares nothing is refused by every other that asks for something.
        ((Write, ShareMode::NONE), (Write, ShareMode::ALL), true),
    ];
    for (index, (first, second, refused)) in cases.into_iter().enumerate() {
        let temp = TempFile::new(&format!("share-named{index}"));
        let _held = open(&temp.0, first.0, first.1).expect("the first open");
        assert_eq!(
            open(&temp.0, second.0, second.1).is_err(),
            refused,
            "{first:?} then {second:?}"
        );
    }
}

#[test]
fn an_open_that_shares_nothing_is_exclusive_to_everything_that_asks_for_data_or_delete() {
    let temp = TempFile::new("share-exclusive");
    let exclusive = OpenedFile::<Cached, ReadWrite>::open(
        wtf(&temp.0),
        FileOptions::new().read().write().share(ShareMode::NONE),
    )
    .expect("the first open of an unopened file");
    for wants in WANTS {
        let error = open(&temp.0, wants, ShareMode::ALL)
            .err()
            .unwrap_or_else(|| panic!("{wants:?} was allowed beside an exclusive open"));
        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy, "{wants:?}");
    }
    drop(exclusive);
    for wants in WANTS {
        assert!(
            open(&temp.0, wants, ShareMode::ALL).is_ok(),
            "{wants:?} once the exclusive open is gone"
        );
    }
}

#[test]
fn an_open_that_asks_for_no_data_and_no_right_is_not_judged_by_the_share_mode() {
    let temp = TempFile::new("share-nothing");
    let _exclusive = OpenedFile::<Cached, ReadWrite>::open(
        wtf(&temp.0),
        FileOptions::new().read().write().share(ShareMode::NONE),
    )
    .expect("an exclusive open");
    OpenedFile::<Cached, NoData>::open(wtf(&temp.0), FileOptions::new())
        .expect("an open that asks for nothing can address the file all the same");
}

#[test]
fn a_file_is_shared_for_reading_writing_and_deletion_unless_told_otherwise() {
    let temp = TempFile::new("share-default");
    let _first = OpenedFile::<Cached, ReadWrite>::open(
        wtf(&temp.0),
        FileOptions::new().read().write().rights(FileRights::DELETE),
    )
    .expect("a default open");
    for wants in WANTS {
        assert!(open(&temp.0, wants, ShareMode::ALL).is_ok(), "{wants:?}");
    }
}

#[test]
fn the_error_names_what_this_open_asked_for_and_keeps_the_kernels_error() {
    let temp = TempFile::new("share-error");
    let _held = OpenedFile::<Cached, ReadWrite>::open(
        wtf(&temp.0),
        FileOptions::new().read().write().share(ShareMode::NONE),
    )
    .expect("an exclusive open");
    let error = OpenedFile::<Cached, ReadWrite>::open(
        wtf(&temp.0),
        FileOptions::new()
            .read()
            .write()
            .rights(FileRights::DELETE)
            .share(ShareMode::READ),
    )
    .expect_err("an exclusive open is open");
    let violation = sharing_violation(&error);
    assert_eq!(violation.by, OpenedBy::Path(temp.0.clone()));
    assert_eq!(violation.data, Some(Access::ReadWrite));
    assert_eq!(violation.rights, FileRights::DELETE);
    assert_eq!(violation.share, ShareMode::READ);
    assert_eq!(
        kernel_code(violation),
        Some(ERROR_SHARING_VIOLATION.cast_signed())
    );
    assert!(error.to_string().contains("conflicts"), "{error}");
}

#[test]
fn a_failure_that_is_not_a_sharing_violation_is_the_kernels_error_as_it_was() {
    let temp = TempFile::new("share-other");
    let missing = temp.0.with_extension("missing");
    let error = OpenedFile::<Cached, ReadWrite>::open(
        wtf(&missing),
        FileOptions::new().read().write().share(ShareMode::NONE),
    )
    .expect_err("there is no such file");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(
        error.get_ref().is_none(),
        "no wrapper: the kernel's own error"
    );
}

#[test]
fn the_data_access_of_each_marker_is_named_as_a_value_and_no_data_is_none() {
    use super::super::{ReadOnly, WriteOnly};
    assert_eq!(data_access::<NoData>(), None);
    assert_eq!(data_access::<ReadOnly>(), Some(Access::ReadOnly));
    assert_eq!(data_access::<WriteOnly>(), Some(Access::WriteOnly));
    assert_eq!(data_access::<ReadWrite>(), Some(Access::ReadWrite));
}

#[test]
fn the_share_mode_survives_the_options_changing_their_data_access() {
    let temp = TempFile::new("share-types");
    // `share` is set before `read` and `write`, which rebuild the options for the new access type.
    let _exclusive = OpenedFile::<Cached, ReadWrite>::open(
        wtf(&temp.0),
        FileOptions::new().share(ShareMode::NONE).read().write(),
    )
    .expect("an exclusive open");
    assert!(open(&temp.0, Wants::Read, ShareMode::ALL).is_err());
}

#[test]
fn the_share_mode_reaches_the_request_as_it_is_and_defaults_to_every_member() {
    let share_of = |options: FileOptions<ReadWrite>| options.parameters(0).share;
    let read_write = || FileOptions::new().read().write();
    assert_eq!(share_of(read_write()), ShareMode::ALL.bits());
    for mode in every_share_mode() {
        assert_eq!(share_of(read_write().share(mode)), mode.bits(), "{mode:?}");
    }
    assert_eq!(
        share_of(read_write().share(ShareMode::NONE).share(ShareMode::READ)),
        ShareMode::READ.bits(),
        "the last call replaces the mode"
    );
}
