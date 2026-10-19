// Copyright (c) 2026 Mike Grier
//! Tests for the open by identifier: each kind of identifier opens the file it names, from a hint of
//! each kind; what the options ask the kernel for; and what is refused. What the kernel does with an
//! identifier that names nothing, a directory and an inheritable handle is what was observed on the
//! machine this was written on, and is recorded in `DI-D-70`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};
use std::process::Command;

use win_handles_sys::{HandleFlags, get_handle_information};
use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::{Event, ResetMode};
use windows_file_enumeration_sys::{Completion, EnumerationRequest, FileIdentityMode, Session};
use windows_namespace_request_sys::file_info::QueryFileInformationByHandle;
use windows_namespace_request_sys::open::OpenFile;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OPEN_NO_RECALL,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED, FILE_FLAG_RANDOM_ACCESS,
    FILE_FLAG_SEQUENTIAL_SCAN, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_WRITE_DATA, OPEN_EXISTING,
};
use wtf_string::Wtf16String;

use super::{ByIdOptions, IdentityMismatch};
use crate::dioring::tests::{TempFile, wtf};
use crate::windows_backend::{
    Cached, FileId, FileIdentifier, FileOptions, FileRights, NoData, OpenedBy, OpenedFile,
    ReadOnly, ReadWrite, ShareMode, SharingViolation, Unbuffered,
};
use durable_ioring::{Access, Geometry};

/// `ERROR_INVALID_PARAMETER`, which an unbuffered file answers an unaligned transfer with.
const INVALID_PARAMETER: i32 = 87;

/// `ERROR_ACCESS_DENIED`.
const ACCESS_DENIED: i32 = 5;

fn read_write() -> ByIdOptions<ReadWrite> {
    ByIdOptions::new().read().write()
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

/// A handle to `directory`, which `FILE_FLAG_BACKUP_SEMANTICS` lets std open.
fn directory_handle(directory: &Path) -> SharedHandle {
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(directory)
        .map(SharedHandle::from)
        .expect("open the directory")
}

/// The directory `TempFile`s are made in, on the volume the tests' files are on.
fn temp_directory() -> SharedHandle {
    directory_handle(&std::env::temp_dir())
}

fn contents_through<B: crate::windows_backend::Buffering, A: durable_ioring::DataAccess>(
    file: &OpenedFile<B, A>,
) -> Vec<u8> {
    let mut contents = Vec::new();
    io_through(file)
        .read_to_end(&mut contents)
        .expect("read through the handle");
    contents
}

/// A directory of its own under the temporary directory, removed with what is in it when dropped.
struct Directory(PathBuf);

impl Directory {
    fn new(tag: &str) -> Self {
        let path = TempFile::new(tag).0.with_extension("dir");
        fs::create_dir(&path).expect("create the directory");
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// --- What each kind of identifier opens ---------------------------------------------------------

#[test]
fn a_file_opened_by_path_and_again_by_its_identity_is_the_same_file() {
    let temp = TempFile::new("bi-round-trip");
    let by_path = temp.open();
    let id = by_path.file_id().expect("the identity");

    let by_id = OpenedFile::<Cached, _>::open_by_id(temp_directory(), id, read_write())
        .expect("open by identity");
    assert_eq!(by_id.file_id().expect("its identity"), id);

    io_through(&by_id)
        .write_all(b"written by id")
        .expect("write through the handle opened by identity");
    let mut seen = String::new();
    let mut through_path = io_through(&by_path);
    through_path.seek(SeekFrom::Start(0)).expect("rewind");
    through_path.read_to_string(&mut seen).expect("read back");
    assert_eq!(seen, "written by id");
}

#[test]
fn the_files_own_handle_a_volume_handle_and_a_directory_are_each_a_hint() {
    let temp = TempFile::new("bi-hints");
    let file = temp.open();
    let id = file.file_id().expect("the identity");
    let volume = volume_of(&temp.0);

    for hint in [file.handle().clone(), temp_directory(), volume] {
        let opened = OpenedFile::<Cached, _>::open_by_id(hint, id, ByIdOptions::new().read())
            .expect("open by identity");
        assert_eq!(opened.file_id().expect("its identity"), id);
    }
}

/// A handle to the volume `path` is on, opened asking for no access, which the hint needs none of.
fn volume_of(path: &Path) -> SharedHandle {
    let drive: String = path.to_string_lossy().chars().take(2).collect();
    assert!(
        drive.ends_with(':'),
        "the temporary directory is on a drive letter: {}",
        path.display()
    );
    OpenFile::new(Wtf16String::from(format!(r"\\.\{drive}").as_str()))
        .expect("the path has no NUL")
        .with_desired_access(0)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .with_creation_disposition(OPEN_EXISTING)
        .perform()
        .map(SharedHandle::from)
        .expect("open the volume asking for no access")
}

#[test]
fn the_64_bit_file_id_opens_the_file() {
    let temp = TempFile::new("bi-64");
    fs::write(&temp.0, b"sixty-four").expect("write the file");
    let file = temp.open();
    let information = QueryFileInformationByHandle::new(file.handle().clone())
        .perform()
        .expect("GetFileInformationByHandle");
    let id64 = (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);

    let opened = OpenedFile::<Cached, _>::open_by_id(
        temp_directory(),
        FileIdentifier::FileId64(id64),
        ByIdOptions::new().read(),
    )
    .expect("open by the 64-bit file ID");
    assert_eq!(
        opened.file_id().expect("its identity"),
        file.file_id().expect("the file's")
    );
    assert_eq!(contents_through(&opened), b"sixty-four");
}

#[test]
fn an_object_id_as_fsutil_reports_it_opens_the_file() {
    let temp = TempFile::new("bi-object");
    let output = Command::new("fsutil")
        .args(["objectid", "create"])
        .arg(&temp.0)
        .output()
        .expect("run fsutil");
    assert!(
        output.status.success(),
        "fsutil objectid create: {output:?}"
    );
    let object_id = first_hex_identifier(&String::from_utf8_lossy(&output.stdout))
        .expect("fsutil printed an object ID");

    let opened = OpenedFile::<Cached, _>::open_by_id(
        temp_directory(),
        FileIdentifier::ObjectId(object_id),
        ByIdOptions::new().read(),
    )
    .expect("open by object ID");
    assert_eq!(
        opened.file_id().expect("its identity"),
        temp.open().file_id().expect("the file's")
    );
}

/// The first 32 hex digits standing alone in `text`, as bytes in the order printed.
fn first_hex_identifier(text: &str) -> Option<[u8; 16]> {
    let token = text
        .split_whitespace()
        .find(|token| token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit()))?;
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&token[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

#[test]
fn an_identity_from_a_directory_listing_opens_the_entry() {
    let directory = Directory::new("bi-listing");
    let path = directory.0.join("entry.txt");
    fs::write(&path, b"listed").expect("create the entry");

    let (session, receiver) = Session::new(8, 8).expect("a session");
    let request = EnumerationRequest::for_path(&directory.0)
        .expect("a request")
        .with_file_identity(FileIdentityMode::Required);
    session.try_begin(request).expect("begin").detach();
    let mut identity = None;
    while let Some(completion) = receiver.recv() {
        match completion {
            Completion::Entry { entry, .. } if entry.name().to_os_string() == "entry.txt" => {
                identity = Some(entry.identity());
            }
            Completion::Entry { .. } => {}
            Completion::Terminal { .. } => break,
        }
    }
    let identity = identity.expect("the listing has the entry");
    let id = FileId::new(
        identity
            .volume_serial()
            .expect("the identity is volume-qualified"),
        identity.id_bytes(),
    );

    let opened = OpenedFile::<Cached, _>::open_by_id(
        directory_handle(&directory.0),
        id,
        ByIdOptions::new().read(),
    )
    .expect("open the listed identity");
    assert_eq!(contents_through(&opened), b"listed");
    drop(opened);
    drop(session);
}

// --- What is refused ----------------------------------------------------------------------------

#[test]
fn an_identity_whose_volume_is_not_the_hints_is_refused_and_nothing_is_held() {
    let temp = TempFile::new("bi-mismatch");
    let real = temp.open().file_id().expect("the identity");
    let asked = FileId::new(real.volume_serial_number() ^ 1, real.id());

    let error =
        OpenedFile::<Cached, _>::open_by_id(temp_directory(), asked, ByIdOptions::new().read())
            .expect_err("the hint's volume is not the identity's");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    let mismatch = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<IdentityMismatch>())
        .expect("the cause is an IdentityMismatch");
    assert_eq!(
        *mismatch,
        IdentityMismatch {
            asked,
            opened: real
        }
    );
    assert!(
        !temp.is_held(),
        "the handle the refused open made is closed"
    );
}

#[test]
fn the_identity_asked_for_on_its_own_volume_passes_the_check_even_with_no_access() {
    // The check reads the opened file's identity, which needs no data right, and compares the whole
    // identity: the same identity on the same volume opens.
    let temp = TempFile::new("bi-match");
    let id = temp.open().file_id().expect("the identity");
    OpenedFile::<Cached, NoData>::open_by_id(temp_directory(), id, ByIdOptions::new())
        .expect("the identity asked for is the one opened");
}

#[test]
fn an_identity_that_names_nothing_is_the_kernels_error() {
    let temp = TempFile::new("bi-stale");
    let id = temp.open().file_id().expect("the identity");
    let path = temp.0.clone();
    drop(temp);
    assert!(!path.exists(), "the file is gone");

    let error =
        OpenedFile::<Cached, _>::open_by_id(temp_directory(), id, ByIdOptions::new().read())
            .expect_err("nothing has that identity");
    assert!(
        error.raw_os_error().is_some(),
        "the kernel's code, unaltered: {error:?}"
    );
    assert!(
        error.get_ref().is_none(),
        "not one of this crate's errors: {error:?}"
    );
}

#[test]
fn a_directory_was_observed_to_open_by_identity_where_by_path_it_is_refused() {
    // `FILE_FLAG_BACKUP_SEMANTICS`, which the `OpenFileById` page says "You must set ... to obtain a
    // handle to a directory", is not in v1, and an open by path of a directory without it is refused
    // by the kernel. An open by identity was observed to open the directory anyway, for every data
    // access and in either buffering mode. Nothing here refuses it: whether an `OpenedFile` may be a
    // directory is `DI-15.5`'s question.
    let directory = Directory::new("bi-directory");
    let id = FileId::of(&directory_handle(&directory.0)).expect("the directory's identity");

    let by_path = OpenedFile::<Cached, NoData>::open(wtf(&directory.0), FileOptions::new())
        .expect_err("a directory is not opened by path without the backup flag");
    assert_eq!(by_path.raw_os_error(), Some(ACCESS_DENIED));

    let opened = [
        OpenedFile::<Cached, NoData>::open_by_id(temp_directory(), id, ByIdOptions::new())
            .and_then(|file| file.file_id()),
        OpenedFile::<Cached, ReadOnly>::open_by_id(temp_directory(), id, ByIdOptions::new().read())
            .and_then(|file| file.file_id()),
        OpenedFile::<Cached, ReadWrite>::open_by_id(temp_directory(), id, read_write())
            .and_then(|file| file.file_id()),
        OpenedFile::<Unbuffered, ReadOnly>::open_by_id(
            temp_directory(),
            id,
            ByIdOptions::new().read(),
        )
        .and_then(|file| file.file_id()),
    ];
    for result in opened {
        assert_eq!(result.expect("the directory opens by identity"), id);
    }
}

#[test]
fn a_hint_that_is_not_on_a_file_system_is_the_kernels_error() {
    let temp = TempFile::new("bi-event-hint");
    let id = temp.open().file_id().expect("the identity");
    let event = Event::new(ResetMode::Manual, false).expect("an event");
    let error = OpenedFile::<Cached, _>::open_by_id(event, id, ByIdOptions::new().read())
        .expect_err("an event is on no volume");
    assert!(
        error.raw_os_error().is_some(),
        "the kernel's code, unaltered: {error:?}"
    );
}

#[test]
fn an_open_that_conflicts_with_the_sharing_of_one_already_open_is_a_sharing_violation() {
    let temp = TempFile::new("bi-share");
    let exclusive = OpenedFile::<Cached, _>::open(
        wtf(&temp.0),
        FileOptions::new().read().share(ShareMode::NONE),
    )
    .expect("an exclusive open");
    let id = exclusive.file_id().expect("the identity");

    let error = OpenedFile::<Cached, _>::open_by_id(
        temp_directory(),
        id,
        ByIdOptions::new()
            .read()
            .rights(FileRights::READ_ATTRIBUTES),
    )
    .expect_err("the first open shares nothing");
    assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
    let violation = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<SharingViolation>())
        .expect("the cause is a SharingViolation");
    assert_eq!(
        violation.by,
        OpenedBy::Identifier(FileIdentifier::Extended(id))
    );
    assert_eq!(violation.data, Some(Access::ReadOnly));
    assert_eq!(violation.rights, FileRights::READ_ATTRIBUTES);
    assert_eq!(violation.share, ShareMode::ALL);
    assert!(
        violation.to_string().contains("the file identified by"),
        "{violation}"
    );
}

#[test]
fn a_file_deleted_by_name_while_held_was_observed_to_open_by_identity() {
    // The page: "If you call OpenFileById on a file that is pending deletion as a result of a
    // previous call to DeleteFile, the function fails", with ERROR_ACCESS_DENIED. Here the file is
    // held by a handle that shares deletion and deleted by name with `std::fs::remove_file`; its
    // name is then gone, and an open by identity was observed to open it. What state the deletion
    // left the file in was not examined, and the page is the authority. Nothing here depends on it.
    let temp = TempFile::new("bi-pending");
    let held = temp.open();
    let id = held.file_id().expect("the identity");
    fs::remove_file(&temp.0).expect("delete while it is held");
    assert!(!temp.0.exists(), "the name is gone");

    let opened =
        OpenedFile::<Cached, NoData>::open_by_id(held.handle().clone(), id, ByIdOptions::new())
            .expect("observed to open");
    assert_eq!(opened.file_id().expect("its identity"), id);
}

// --- What the handle is -------------------------------------------------------------------------

#[test]
fn the_kernel_allows_the_data_access_asked_for_and_no_other() {
    let temp = TempFile::new("bi-access");
    let id = temp.open().file_id().expect("the identity");
    let read_only =
        OpenedFile::<Cached, ReadOnly>::open_by_id(temp_directory(), id, ByIdOptions::new().read())
            .expect("open read-only");
    let mut through = io_through(&read_only);
    let mut byte = [0; 1];
    through
        .read_exact(&mut byte)
        .expect("a read-only handle reads");
    let refused = through
        .write_all(b"no")
        .expect_err("a read-only handle does not write");
    assert_eq!(refused.raw_os_error(), Some(ACCESS_DENIED));
}

#[test]
fn an_unbuffered_open_by_identity_is_unbuffered_to_the_kernel_and_needs_a_sector() {
    let temp = TempFile::new("bi-unbuffered");
    let id = temp.open().file_id().expect("the identity");
    let unbuffered = OpenedFile::<Unbuffered, _>::open_by_id(temp_directory(), id, read_write())
        .expect("open unbuffered by identity");
    assert_eq!(
        unbuffered.geometry(),
        Geometry::uniform(Unbuffered::DEFAULT_SECTOR_SIZE).expect("a power of two")
    );
    let mut through = io_through(&unbuffered);
    let refused = through
        .seek(SeekFrom::Start(3))
        .and_then(|_| through.write_all(b"seven b"))
        .expect_err("an unbuffered handle refuses an unaligned write");
    assert_eq!(refused.raw_os_error(), Some(INVALID_PARAMETER));

    let cached = OpenedFile::<Cached, _>::open_by_id(temp_directory(), id, read_write())
        .expect("open cached by identity");
    assert_eq!(cached.geometry(), Geometry::UNALIGNED);
    let mut through = io_through(&cached);
    through
        .seek(SeekFrom::Start(3))
        .and_then(|_| through.write_all(b"seven b"))
        .expect("a cached handle takes it");
}

#[test]
fn the_handle_an_open_by_identity_makes_is_not_inheritable() {
    // The page does not say; the request passes no security attributes, as the page marks the
    // parameter reserved, and the handle that results was observed not inheritable.
    let temp = TempFile::new("bi-inherit");
    let id = temp.open().file_id().expect("the identity");
    let opened =
        OpenedFile::<Cached, _>::open_by_id(temp_directory(), id, ByIdOptions::new().read())
            .expect("open by identity");
    assert_eq!(
        get_handle_information(opened.handle().as_handle()).expect("query"),
        HandleFlags::NONE
    );
}

#[test]
fn a_request_is_built_on_one_thread_and_performed_on_another_and_again() {
    let temp = TempFile::new("bi-thread");
    let id = temp.open().file_id().expect("the identity");
    let hint = temp_directory();
    let request =
        OpenedFile::<Cached, ReadOnly>::request_by_id(hint.clone(), id, ByIdOptions::new().read());
    assert_eq!(request.identifier(), FileIdentifier::Extended(id));
    drop(hint);

    let request = std::thread::spawn(move || {
        let first = request.perform().expect("performed on another thread");
        let second = request.perform().expect("and again");
        assert_eq!(first.file_id().expect("first"), id);
        assert_eq!(second.file_id().expect("second"), id);
        request
    })
    .join()
    .expect("the thread");
    assert_eq!(request.identifier(), FileIdentifier::Extended(id));
}

// --- What the options ask for -------------------------------------------------------------------

#[test]
fn each_option_sets_its_own_flag_and_no_other() {
    let base = ByIdOptions::new();
    assert_eq!(base.flags(0), 0);
    let cases: [(ByIdOptions<NoData>, u32); 5] = [
        (base.open_reparse_point(true), FILE_FLAG_OPEN_REPARSE_POINT),
        (base.open_no_recall(true), FILE_FLAG_OPEN_NO_RECALL),
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
        .open_no_recall(true)
        .overlapped(true)
        .random_access(true)
        .sequential_scan(true);
    assert_eq!(all.flags(0), every);
    assert_eq!(
        all.open_no_recall(false).flags(0),
        every & !FILE_FLAG_OPEN_NO_RECALL
    );
}

#[test]
fn the_access_is_the_data_access_and_the_other_rights_and_survives_a_change_of_access() {
    let base = ByIdOptions::new();
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

    let with_rights = base
        .rights(FileRights::READ_ATTRIBUTES)
        .share(ShareMode::READ)
        .sequential_scan(true);
    let moved = with_rights.read().write();
    assert_eq!(
        moved.access(),
        FILE_READ_DATA | FILE_WRITE_DATA | FileRights::READ_ATTRIBUTES.bits()
    );
    assert_eq!(moved.share, ShareMode::READ);
    assert_eq!(moved.flags(0), FILE_FLAG_SEQUENTIAL_SCAN);
}

#[test]
fn a_file_is_shared_for_everything_unless_told_otherwise() {
    assert_eq!(ByIdOptions::new().share, ShareMode::ALL);
    assert_eq!(ByIdOptions::default().share, ShareMode::ALL);
    assert_eq!(
        ByIdOptions::new().share(ShareMode::NONE).share,
        ShareMode::NONE
    );
}

#[test]
fn the_request_carries_what_the_options_and_the_buffering_mode_ask_for() {
    let temp = TempFile::new("bi-request");
    let options = ByIdOptions::new()
        .read()
        .rights(FileRights::READ_ATTRIBUTES)
        .share(ShareMode::READ | ShareMode::DELETE)
        .random_access(true);
    let id = FileIdentifier::FileId64(42);
    let cached = OpenedFile::<Cached, ReadOnly>::request_by_id(temp_directory(), id, options);
    let unbuffered =
        OpenedFile::<Unbuffered, ReadOnly>::request_by_id(temp_directory(), id, options);
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
        assert!(request.security().is_none(), "the parameter is reserved");
    }
    drop(temp);
}
