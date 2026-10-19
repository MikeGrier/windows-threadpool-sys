// Copyright (c) 2026 Mike Grier
//! Tests for the reopen: what it makes of an owned file and of a handle opened elsewhere, in each
//! buffering mode and data access; the documented sharing rule over every pair of an original and a
//! reopen, judged by the share-mode tests' own statement of it; and what the kernel does with the
//! originals the page leaves open, which is what was observed on the machine this was written on and
//! is recorded in `DI-D-71`.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::path::Path;

use win_handles_sys::{HandleFlags, get_handle_information};
use win_shared_os_owned_handle::SharedHandle;
use win_volume_sys::storage_device_number;
use windows_namespace_request_sys::open::OpenFile;
use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OPEN_NO_RECALL,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED, FILE_FLAG_POSIX_SEMANTICS,
    FILE_FLAG_RANDOM_ACCESS, FILE_FLAG_SEQUENTIAL_SCAN, FILE_READ_DATA, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA, OPEN_EXISTING,
};
use wtf_string::Wtf16String;

use super::super::share_tests::{
    WANTS, Wants, documented_to_conflict, every_share_mode, kernel_code, sharing_violation,
};
use super::super::tests::admin_share;
use super::ReopenOptions;
use crate::dioring::tests::{TempFile, wtf};
use crate::windows_backend::{
    ByIdOptions, Cached, FileOptions, FileRights, NoData, OpenedBy, OpenedFile, ReadOnly,
    ReadWrite, RemoteDevice, ShareMode, Unbuffered, UnknownLocation, WriteOnly,
};
use durable_ioring::Geometry;

/// `ERROR_INVALID_PARAMETER`, which an unbuffered file answers an unaligned transfer with.
const INVALID_PARAMETER: i32 = 87;

/// `ERROR_ACCESS_DENIED`.
const ACCESS_DENIED: i32 = 5;

fn read_write() -> ReopenOptions<ReadWrite> {
    ReopenOptions::new().read().write()
}

/// A second handle to what `opened` holds, as a `std::fs::File`, so the kernel judges the I/O.
fn io_through(opened: &impl AsHandle) -> File {
    File::from(
        opened
            .as_handle()
            .try_clone_to_owned()
            .expect("duplicate the handle"),
    )
}

/// The same unaligned seven-byte write at offset three: a cached handle takes it, and the kernel
/// refuses it on an unbuffered one.
fn unaligned_write(opened: &impl AsHandle) -> io::Result<()> {
    let mut file = io_through(opened);
    file.seek(SeekFrom::Start(3))?;
    file.write_all(b"seven b")
}

/// A handle to a device opened by `path`, asking for `access` and sharing everything.
fn device(path: &str, access: u32) -> SharedHandle {
    OpenFile::new(Wtf16String::from(path))
        .expect("the path has no NUL")
        .with_desired_access(access)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .with_creation_disposition(OPEN_EXISTING)
        .perform()
        .map(SharedHandle::from)
        .unwrap_or_else(|error| panic!("open {path}: {error:?}"))
}

/// The volume the temporary directory is on, as `\\.\X:`.
fn temp_volume() -> String {
    let temp = std::env::temp_dir();
    let drive: String = temp.to_string_lossy().chars().take(2).collect();
    assert!(drive.ends_with(':'), "a drive letter: {}", temp.display());
    format!(r"\\.\{drive}")
}

// --- An owned file, reopened ----------------------------------------------------------------------

#[test]
fn a_cached_file_reopens_unbuffered_and_back_and_the_kernel_treats_each_as_its_type_says() {
    let temp = TempFile::new("ro-modes");
    fs::write(&temp.0, [0u8; 8192]).expect("size the file");
    let cached = temp.open();
    unaligned_write(&cached).expect("a cached handle takes an unaligned write");

    let unbuffered = OpenedFile::<Unbuffered, _>::reopen(cached.handle().clone(), read_write())
        .expect("reopen unbuffered");
    assert_eq!(
        unbuffered.geometry(),
        Geometry::uniform(Unbuffered::DEFAULT_SECTOR_SIZE).expect("a power of two"),
        "the alignment is recomputed for the new mode"
    );
    let refused = unaligned_write(&unbuffered).expect_err("an unbuffered handle refuses it");
    assert_eq!(refused.raw_os_error(), Some(INVALID_PARAMETER));

    let back = OpenedFile::<Cached, _>::reopen(unbuffered.handle().clone(), read_write())
        .expect("reopen cached from the unbuffered handle");
    assert_eq!(back.geometry(), Geometry::UNALIGNED);
    unaligned_write(&back).expect("cached again");
    assert_eq!(
        back.file_id().expect("its identity"),
        cached.file_id().expect("the file's")
    );
}

#[test]
fn a_stated_sector_size_is_not_carried_to_a_reopen() {
    let temp = TempFile::new("ro-sector");
    let original = OpenedFile::<Unbuffered, _>::open(wtf(&temp.0), FileOptions::new().read())
        .expect("open unbuffered")
        .with_sector_size(512)
        .expect("a power of two");
    let reopened =
        OpenedFile::<Unbuffered, _>::reopen(original.handle().clone(), ReopenOptions::new().read())
            .expect("reopen");
    assert_eq!(
        original.geometry(),
        Geometry::uniform(512).expect("a power of two")
    );
    assert_eq!(
        reopened.geometry(),
        Geometry::uniform(Unbuffered::DEFAULT_SECTOR_SIZE).expect("a power of two")
    );
}

#[test]
fn a_read_only_handle_beside_the_writer_reads_what_it_wrote_and_cannot_write() {
    let temp = TempFile::new("ro-scrubber");
    let writer = temp.open();
    let scrubber = OpenedFile::<Cached, ReadOnly>::reopen(
        writer.handle().clone(),
        ReopenOptions::new().read(),
    )
    .expect("a read-only reopen beside the writer");

    io_through(&writer).write_all(b"written").expect("write");
    let mut seen = String::new();
    io_through(&scrubber)
        .read_to_string(&mut seen)
        .expect("the scrubber reads");
    assert_eq!(seen, "written");
    let refused = io_through(&scrubber)
        .write_all(b"no")
        .expect_err("a read-only handle does not write");
    assert_eq!(refused.raw_os_error(), Some(ACCESS_DENIED));
}

#[test]
fn a_reopen_widens_read_only_to_read_write_where_the_files_security_allows() {
    // The request crate's tests show a reopen widening read to write on a `std` file; this is the same
    // on this crate's types. The access is checked against the caller afresh (`DI-D-68`'s broker
    // case), so a file the caller may write reopens for writing from a read-only handle.
    let temp = TempFile::new("ro-widen");
    let reader = OpenedFile::<Cached, ReadOnly>::open(wtf(&temp.0), FileOptions::new().read())
        .expect("open read-only");
    let widened = OpenedFile::<Cached, ReadWrite>::reopen(reader.handle().clone(), read_write())
        .expect("widen to read and write");
    io_through(&widened)
        .write_all(b"widened")
        .expect("the reopened handle writes");
    assert_eq!(fs::read(&temp.0).expect("read back"), b"widened");
}

#[test]
fn each_data_access_reopens_and_the_kernel_allows_what_it_names() {
    let temp = TempFile::new("ro-access");
    let original = temp.open();
    let none =
        OpenedFile::<Cached, NoData>::reopen(original.handle().clone(), ReopenOptions::new())
            .expect("no data");
    let read = OpenedFile::<Cached, ReadOnly>::reopen(
        original.handle().clone(),
        ReopenOptions::new().read(),
    )
    .expect("read");
    let write = OpenedFile::<Cached, WriteOnly>::reopen(
        original.handle().clone(),
        ReopenOptions::new().write(),
    )
    .expect("write");
    let mut byte = [0; 1];
    assert_eq!(
        io_through(&none)
            .read(&mut byte)
            .map_err(|e| e.raw_os_error()),
        Err(Some(ACCESS_DENIED))
    );
    io_through(&read)
        .read_exact(&mut byte)
        .expect("a read-only handle reads");
    io_through(&write)
        .write_all(b"w")
        .expect("a write-only handle writes");
    assert_eq!(
        io_through(&write)
            .read(&mut byte)
            .map_err(|e| e.raw_os_error()),
        Err(Some(ACCESS_DENIED))
    );
}

#[test]
fn delete_on_close_given_to_a_reopen_deletes_the_file_when_every_handle_is_closed() {
    let temp = TempFile::new("ro-delete");
    let original = temp.open();
    let doomed = OpenedFile::<Cached, ReadOnly>::reopen(
        original.handle().clone(),
        ReopenOptions::new().read().delete_on_close(true),
    )
    .expect("reopen to delete on close");
    drop(doomed);
    assert!(temp.0.exists(), "the original still holds the file");
    drop(original);
    assert!(!temp.0.exists(), "gone with its last handle");
}

// --- Any handle, reopened ----------------------------------------------------------------------

#[test]
fn a_handle_opened_by_std_reopens_into_a_file_of_this_crates_type() {
    let temp = TempFile::new("ro-std");
    let std_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temp.0)
        .expect("open with std");
    let unbuffered = OpenedFile::<Unbuffered, _>::reopen(std_file, read_write())
        .expect("a foreign handle reopens unbuffered");
    let refused = unaligned_write(&unbuffered).expect_err("and is unbuffered to the kernel");
    assert_eq!(refused.raw_os_error(), Some(INVALID_PARAMETER));
}

#[test]
fn a_handle_from_an_open_by_identity_reopens() {
    let temp = TempFile::new("ro-by-id");
    let original = temp.open();
    let id = original.file_id().expect("the identity");
    let by_id = OpenedFile::<Cached, _>::open_by_id(
        original.handle().clone(),
        id,
        ByIdOptions::new().read().write(),
    )
    .expect("open by identity");
    // `DI-D-64`'s unverified list asked whether a handle from `OpenFileById` reopens; the page says
    // the original must have been created by `CreateFile`. Observed to reopen.
    let reopened = OpenedFile::<Unbuffered, ReadOnly>::reopen(
        by_id.handle().clone(),
        ReopenOptions::new().read(),
    )
    .expect("observed to reopen");
    assert_eq!(reopened.file_id().expect("its identity"), id);
}

#[test]
fn the_original_need_not_outlive_the_request() {
    let temp = TempFile::new("ro-request");
    let original = temp.open();
    let request = OpenedFile::<Cached, ReadOnly>::request_reopen(
        original.handle().clone(),
        ReopenOptions::new().read(),
    );
    let id = original.file_id().expect("the identity");
    drop(original);
    let request = std::thread::spawn(move || {
        let first = request.perform().expect("performed on another thread");
        let second = request.perform().expect("and again");
        assert_eq!(first.file_id().expect("first"), id);
        assert_eq!(second.file_id().expect("second"), id);
        request
    })
    .join()
    .expect("the thread");
    drop(request);
}

#[test]
fn the_reopened_handle_is_not_inheritable() {
    let temp = TempFile::new("ro-inherit");
    let original = temp.open();
    let reopened = OpenedFile::<Cached, ReadOnly>::reopen(
        original.handle().clone(),
        ReopenOptions::new().read(),
    )
    .expect("reopen");
    assert_eq!(
        get_handle_information(reopened.handle().as_handle()).expect("query"),
        HandleFlags::NONE
    );
}

// --- What the checks on the reopened handle refuse ------------------------------------------------

#[test]
fn nul_reopens_cached_and_is_refused_unbuffered_as_a_handle_of_unknown_location() {
    // The check a reopen from any handle can meet: the original is a device, the reopened handle has
    // no final path, and an unbuffered file is refused for it, as an open by path of `NUL` is.
    let nul = OpenedFile::<Cached>::open(wtf(Path::new("NUL")), FileOptions::new().read().write())
        .expect("open NUL cached");
    OpenedFile::<Cached, _>::reopen(nul.handle().clone(), read_write())
        .expect("a cached reopen of NUL was observed to succeed, and is not refused here");
    let error = OpenedFile::<Unbuffered, _>::reopen(nul.handle().clone(), read_write())
        .expect_err("an unbuffered reopen of NUL");
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    let unknown = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<UnknownLocation>())
        .expect("the cause is an UnknownLocation");
    assert!(unknown.cause.raw_os_error().is_some(), "{unknown:?}");
}

#[test]
fn a_volume_and_a_disk_were_observed_not_to_reopen_where_a_file_opened_with_no_access_does() {
    // The page says the original must have been created by `CreateFile`, which a volume's and a
    // disk's handles were. Unelevated, a reopen of either was refused by the kernel whatever the
    // original or the reopen asked for, while a file opened with no access reopens: the refusal is
    // the device's, not the access's. Elevated, and with a data access, was not measured.
    let temp = TempFile::new("ro-devices");
    let file = OpenedFile::<Cached, NoData>::open(wtf(&temp.0), FileOptions::new()).expect("open");
    OpenedFile::<Cached, NoData>::reopen(file.handle().clone(), ReopenOptions::new())
        .expect("a file opened with no access reopens");
    OpenedFile::<Cached, ReadOnly>::reopen(file.handle().clone(), ReopenOptions::new().read())
        .expect("and reopens for reading");

    let read_attributes = FileRights::READ_ATTRIBUTES.bits();
    let volume = device(&temp_volume(), 0);
    let number = storage_device_number(&volume).expect("the volume's disk");
    let disk = device(&format!(r"\\.\PhysicalDrive{}", number.device_number), 0);
    for (what, handle) in [
        ("a volume", volume.clone()),
        (
            "a volume opened to read attributes",
            device(&temp_volume(), read_attributes),
        ),
        ("a disk", disk),
    ] {
        for result in [
            OpenedFile::<Cached, NoData>::reopen(handle.clone(), ReopenOptions::new()).map(drop),
            OpenedFile::<Unbuffered, NoData>::reopen(handle.clone(), ReopenOptions::new())
                .map(drop),
            OpenedFile::<Cached, NoData>::reopen(
                handle.clone(),
                ReopenOptions::new().rights(FileRights::READ_ATTRIBUTES),
            )
            .map(drop),
        ] {
            let error = result.expect_err(what);
            assert!(
                error.raw_os_error().is_some(),
                "{what}: the kernel's code: {error:?}"
            );
        }
    }
}

#[test]
#[ignore = "needs the administrative share \\\\localhost\\C$, which a CI runner has"]
fn a_file_on_a_share_reopens_cached_and_is_refused_unbuffered() {
    let temp = TempFile::new("ro-remote");
    let remote = admin_share(&temp.0);
    let on_share = OpenedFile::<Cached>::open(wtf(&remote), FileOptions::new().read().write())
        .expect("a cached file on a share");

    OpenedFile::<Cached, _>::reopen(on_share.handle().clone(), read_write())
        .expect("a cached reopen on a share is allowed");
    let error = OpenedFile::<Unbuffered, _>::reopen(on_share.handle().clone(), read_write())
        .expect_err("an unbuffered reopen on a share is refused");
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(
        error
            .get_ref()
            .is_some_and(|inner| inner.is::<RemoteDevice>()),
        "{error:?}"
    );
}

// --- Sharing ---------------------------------------------------------------------------------------

/// An original opened by path asking for `wants` and sharing `share`, as its shared handle.
fn original(path: &Path, wants: Wants, share: ShareMode) -> SharedHandle {
    let opened = match wants {
        Wants::Read => {
            OpenedFile::<Cached, _>::open(wtf(path), FileOptions::new().read().share(share))
                .map(|file| file.handle().clone())
        }
        Wants::Write => {
            OpenedFile::<Cached, _>::open(wtf(path), FileOptions::new().write().share(share))
                .map(|file| file.handle().clone())
        }
        Wants::Delete => OpenedFile::<Cached, NoData>::open(
            wtf(path),
            FileOptions::new().rights(FileRights::DELETE).share(share),
        )
        .map(|file| file.handle().clone()),
    };
    opened.unwrap_or_else(|error| panic!("the original {wants:?}/{share:?}: {error}"))
}

/// A reopen of `handle` asking for `wants` and sharing `share`.
fn reopened(handle: &SharedHandle, wants: Wants, share: ShareMode) -> io::Result<()> {
    match wants {
        Wants::Read => OpenedFile::<Cached, _>::reopen(
            handle.clone(),
            ReopenOptions::new().read().share(share),
        )
        .map(drop),
        Wants::Write => OpenedFile::<Cached, _>::reopen(
            handle.clone(),
            ReopenOptions::new().write().share(share),
        )
        .map(drop),
        Wants::Delete => OpenedFile::<Cached, NoData>::reopen(
            handle.clone(),
            ReopenOptions::new().rights(FileRights::DELETE).share(share),
        )
        .map(drop),
    }
}

#[test]
fn every_pair_of_an_original_and_a_reopen_is_accepted_or_refused_as_the_documentation_says() {
    let temp = TempFile::new("ro-matrix");
    let modes = every_share_mode();
    let mut refusals = 0;
    for first_wants in WANTS {
        for &first_share in &modes {
            let handle = original(&temp.0, first_wants, first_share);
            for second_wants in WANTS {
                for &second_share in &modes {
                    let outcome = reopened(&handle, second_wants, second_share);
                    let conflict = documented_to_conflict(
                        (first_wants, first_share),
                        (second_wants, second_share),
                    );
                    assert_eq!(
                        outcome.is_err(),
                        conflict,
                        "{first_wants:?} sharing {first_share:?}, then a reopen of {second_wants:?} \
                         sharing {second_share:?}: {outcome:?}"
                    );
                    if let Err(error) = outcome {
                        refusals += 1;
                        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
                        let violation = sharing_violation(&error);
                        assert_eq!(violation.by, OpenedBy::Reopen);
                        assert_eq!(
                            kernel_code(violation),
                            Some(ERROR_SHARING_VIOLATION.cast_signed())
                        );
                    }
                }
            }
        }
    }
    assert!(
        refusals > 0,
        "the matrix reached refusals as well as acceptances"
    );
}

// --- What the options ask for -------------------------------------------------------------------

#[test]
fn each_option_sets_its_own_flag_and_no_other() {
    let base = ReopenOptions::new();
    assert_eq!(base.flags(0), 0);
    let cases: [(ReopenOptions<NoData>, u32); 7] = [
        (base.open_reparse_point(true), FILE_FLAG_OPEN_REPARSE_POINT),
        (base.delete_on_close(true), FILE_FLAG_DELETE_ON_CLOSE),
        (base.open_no_recall(true), FILE_FLAG_OPEN_NO_RECALL),
        (base.posix_semantics(true), FILE_FLAG_POSIX_SEMANTICS),
        (base.overlapped(true), FILE_FLAG_OVERLAPPED),
        (base.random_access(true), FILE_FLAG_RANDOM_ACCESS),
        (base.sequential_scan(true), FILE_FLAG_SEQUENTIAL_SCAN),
    ];
    for (options, flag) in cases {
        assert_eq!(options.flags(0), flag);
        assert_eq!(
            options.flags(FILE_FLAG_NO_BUFFERING),
            flag | FILE_FLAG_NO_BUFFERING
        );
    }
    let every = cases.iter().fold(0, |flags, (_, flag)| flags | flag);
    let all = base
        .open_reparse_point(true)
        .delete_on_close(true)
        .open_no_recall(true)
        .posix_semantics(true)
        .overlapped(true)
        .random_access(true)
        .sequential_scan(true);
    assert_eq!(all.flags(0), every);
    assert_eq!(
        all.delete_on_close(false).flags(0),
        every & !FILE_FLAG_DELETE_ON_CLOSE
    );
}

#[test]
fn the_access_is_the_data_access_and_the_other_rights_and_survives_a_change_of_access() {
    let base = ReopenOptions::new();
    assert_eq!(base.access(), 0);
    assert_eq!(base.read().access(), FILE_READ_DATA);
    assert_eq!(base.write().access(), FILE_WRITE_DATA);
    assert_eq!(
        base.read().write().access(),
        FILE_READ_DATA | FILE_WRITE_DATA
    );
    assert_eq!(
        base.write().read().access(),
        FILE_READ_DATA | FILE_WRITE_DATA
    );

    let configured = base
        .rights(FileRights::READ_ATTRIBUTES)
        .share(ShareMode::READ)
        .delete_on_close(true)
        .posix_semantics(true);
    let moved = configured.read().write();
    assert_eq!(
        moved.access(),
        FILE_READ_DATA | FILE_WRITE_DATA | FileRights::READ_ATTRIBUTES.bits()
    );
    assert_eq!(moved.share, ShareMode::READ);
    assert_eq!(
        moved.flags(0),
        FILE_FLAG_DELETE_ON_CLOSE | FILE_FLAG_POSIX_SEMANTICS
    );
}

#[test]
fn a_reopen_is_shared_for_everything_unless_told_otherwise() {
    assert_eq!(ReopenOptions::new().share, ShareMode::ALL);
    assert_eq!(ReopenOptions::default().share, ShareMode::ALL);
    assert_eq!(
        ReopenOptions::new().share(ShareMode::NONE).share,
        ShareMode::NONE
    );
}

#[test]
fn the_request_carries_what_the_options_and_the_buffering_mode_ask_for() {
    let temp = TempFile::new("ro-request-fields");
    let original = temp.open();
    let options = ReopenOptions::new()
        .read()
        .rights(FileRights::READ_ATTRIBUTES)
        .share(ShareMode::READ | ShareMode::DELETE)
        .random_access(true);
    let cached = OpenedFile::<Cached, ReadOnly>::request_reopen(original.handle().clone(), options);
    let unbuffered =
        OpenedFile::<Unbuffered, ReadOnly>::request_reopen(original.handle().clone(), options);
    for (request, mode) in [
        (&cached.request, 0),
        (&unbuffered.request, FILE_FLAG_NO_BUFFERING),
    ] {
        assert_eq!(
            request.desired_access(),
            FILE_READ_DATA | FileRights::READ_ATTRIBUTES.bits()
        );
        assert_eq!(request.share_mode(), FILE_SHARE_READ | FILE_SHARE_DELETE);
        assert_eq!(
            request.flags_and_attributes(),
            FILE_FLAG_RANDOM_ACCESS | mode
        );
    }
    assert!(
        cached.original().as_raw_handle() == original.handle().as_raw_handle(),
        "the original is held shared, not duplicated"
    );
}
