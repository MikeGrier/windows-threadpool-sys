// Copyright (c) 2026 Mike Grier

use std::io;

use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_DATA, FILE_WRITE_DATA};

use super::{RawOpen, open, raw_open_of};
use crate::dioring::tests::TempFile;
use crate::windows_backend::{
    Buffering, Cached, FileRights, NoData, OpenedBy, ReadOnly, ReadWrite, ShareMode,
    SharingViolation, Unbuffered, WriteOnly,
};
use durable_ioring::{Access, DataAccess};

fn every_share_mode() -> Vec<ShareMode> {
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

#[test]
fn a_device_open_asks_for_the_data_rights_of_its_marker_and_the_flags_of_its_mode() {
    fn asked<B: Buffering, A: DataAccess>() -> RawOpen {
        raw_open_of::<B, A>(ShareMode::ALL)
    }
    let table = [
        (asked::<Unbuffered, NoData>().access, 0),
        (asked::<Unbuffered, ReadOnly>().access, FILE_READ_DATA),
        (asked::<Unbuffered, WriteOnly>().access, FILE_WRITE_DATA),
        (
            asked::<Unbuffered, ReadWrite>().access,
            FILE_READ_DATA | FILE_WRITE_DATA,
        ),
        (
            asked::<Cached, ReadWrite>().access,
            FILE_READ_DATA | FILE_WRITE_DATA,
        ),
    ];
    for (index, (actual, expected)) in table.into_iter().enumerate() {
        assert_eq!(actual, expected, "row {index}");
    }
    assert_eq!(asked::<Unbuffered, ReadWrite>().flags, Unbuffered::FLAGS);
    assert_eq!(asked::<Cached, ReadWrite>().flags, Cached::FLAGS);
    assert_ne!(Unbuffered::FLAGS, Cached::FLAGS, "the two modes differ");
}

#[test]
fn the_share_mode_is_passed_to_the_open_as_it_is() {
    for share in every_share_mode() {
        assert_eq!(
            raw_open_of::<Unbuffered, ReadWrite>(share).share,
            share.bits(),
            "{share:?}"
        );
    }
}

/// What the `CreateFileW` documentation says of two opens, restricted to the accesses a device open
/// has (it offers no delete right): the later open fails when it asks for what the earlier does not
/// share, and when the earlier asked for what the later does not share.
fn documented_to_conflict(first: (bool, bool, ShareMode), second: (bool, bool, ShareMode)) -> bool {
    let (first_read, first_write, first_share) = first;
    let (second_read, second_write, second_share) = second;
    (second_read && !first_share.contains(ShareMode::READ))
        || (second_write && !first_share.contains(ShareMode::WRITE))
        || (first_read && !second_share.contains(ShareMode::READ))
        || (first_write && !second_share.contains(ShareMode::WRITE))
}

/// Open `path` with the access of `kind` (read, write) and `share`, boxed so that two kinds can be
/// held at once. The open is the one a raw volume and a raw disk make, here on a file.
fn open_as(
    path: &std::path::Path,
    (read, write): (bool, bool),
    share: ShareMode,
) -> io::Result<Box<dyn std::any::Any>> {
    Ok(match (read, write) {
        (true, false) => Box::new(open::<Unbuffered, ReadOnly>(path, share)?),
        (false, true) => Box::new(open::<Unbuffered, WriteOnly>(path, share)?),
        _ => Box::new(open::<Unbuffered, ReadWrite>(path, share)?),
    })
}

#[test]
fn every_pair_of_device_opens_is_accepted_or_refused_as_the_documentation_says() {
    let temp = TempFile::new("raw-open-matrix");
    let kinds = [(true, false), (false, true), (true, true)];
    let modes = every_share_mode();
    let mut refused = 0;
    for first_kind in kinds {
        for &first_share in &modes {
            let first = open_as(&temp.0, first_kind, first_share).expect("the first open");
            for second_kind in kinds {
                for &second_share in &modes {
                    let outcome = open_as(&temp.0, second_kind, second_share);
                    let conflict = documented_to_conflict(
                        (first_kind.0, first_kind.1, first_share),
                        (second_kind.0, second_kind.1, second_share),
                    );
                    assert_eq!(
                        outcome.is_err(),
                        conflict,
                        "{first_kind:?} sharing {first_share:?}, then {second_kind:?} sharing {second_share:?}"
                    );
                    if let Err(error) = outcome {
                        refused += 1;
                        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
                    }
                }
            }
            drop(first);
        }
    }
    assert!(
        refused > 0,
        "the matrix reached refusals as well as acceptances"
    );
}

#[test]
fn a_conflict_is_the_same_sharing_violation_a_file_reports() {
    let temp = TempFile::new("raw-open-error");
    let _exclusive =
        open::<Unbuffered, ReadWrite>(&temp.0, ShareMode::NONE).expect("an exclusive open");
    let error = open::<Unbuffered, ReadOnly>(&temp.0, ShareMode::READ)
        .expect_err("an exclusive open is open");
    let violation = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<SharingViolation>())
        .expect("the cause is a SharingViolation");
    assert_eq!(violation.by, OpenedBy::Path(temp.0.clone()));
    assert_eq!(violation.data, Some(Access::ReadOnly));
    assert_eq!(
        violation.rights,
        FileRights::NONE,
        "a device open offers no other right"
    );
    assert_eq!(violation.share, ShareMode::READ);
    let kernel = std::error::Error::source(violation)
        .and_then(|source| source.downcast_ref::<io::Error>())
        .and_then(io::Error::raw_os_error);
    assert_eq!(kernel, Some(ERROR_SHARING_VIOLATION.cast_signed()));
}

#[test]
fn an_open_that_asks_for_no_data_right_is_not_judged_by_the_share_mode() {
    let temp = TempFile::new("raw-open-nodata");
    let _exclusive =
        open::<Unbuffered, ReadWrite>(&temp.0, ShareMode::NONE).expect("an exclusive open");
    open::<Unbuffered, NoData>(&temp.0, ShareMode::NONE)
        .expect("an open that asks for nothing can address the device all the same");
}

#[test]
fn a_failure_that_is_not_a_sharing_violation_is_the_kernels_error_as_it_was() {
    let temp = TempFile::new("raw-open-other");
    let missing = temp.0.with_extension("missing");
    let error = open::<Unbuffered, ReadWrite>(&missing, ShareMode::NONE)
        .expect_err("there is no such file");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(
        error.get_ref().is_none(),
        "no wrapper: the kernel's own error"
    );
}

#[test]
fn the_default_sharing_lets_every_other_open_in() {
    let temp = TempFile::new("raw-open-default");
    let _first = open::<Unbuffered, ReadWrite>(&temp.0, ShareMode::ALL).expect("a default open");
    for kind in [(true, false), (false, true), (true, true)] {
        assert!(open_as(&temp.0, kind, ShareMode::ALL).is_ok(), "{kind:?}");
    }
}
