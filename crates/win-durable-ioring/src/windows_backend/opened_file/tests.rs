// Copyright (c) 2026 Mike Grier

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::path::{Path, PathBuf};

use io_buffers::AlignedBuf;

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_RANDOM_ACCESS, FILE_FLAG_SEQUENTIAL_SCAN,
    OPEN_ALWAYS, OPEN_EXISTING, TRUNCATE_EXISTING,
};

use super::super::Win32FileTarget;
use super::{
    Buffering, Cached, FILE_FLAG_NO_BUFFERING, FileOptions, OpenedFile, RemoteDevice, Unbuffered,
    final_location, names_a_remote_device, open_request,
};
use crate::dioring::tests::TempFile;
use durable_ioring::{Capabilities, Geometry, GeometryError, IoTarget, WarrantyClass};

/// `ERROR_INVALID_PARAMETER`, which an unbuffered file answers an unaligned transfer with.
const INVALID_PARAMETER: i32 = 87;

fn read_write() -> FileOptions {
    FileOptions::new().read(true).write(true)
}

/// A second handle to the file `opened` holds, as a `std::fs::File`, to do I/O the kernel itself
/// judges: what the open mode is, as the kernel enforces it rather than as this crate recorded it.
fn io_through(opened: &impl AsHandle) -> File {
    File::from(
        opened
            .as_handle()
            .try_clone_to_owned()
            .expect("duplicate the handle"),
    )
}

#[test]
fn a_cached_file_claims_best_effort_needs_no_alignment_and_has_no_write_through() {
    assert_eq!(Cached::WARRANTY, WarrantyClass::BestEffort);
    assert_eq!(Cached::CAPABILITIES, Capabilities::NONE);
    assert_eq!(Cached::FLAGS, 0);
    const { assert!(!Cached::LOCAL_DEVICE_ONLY) };
    assert_eq!(Cached::geometry(), Geometry::UNALIGNED);
}

#[test]
fn an_unbuffered_file_claims_shared_device_needs_a_sector_and_offers_write_through() {
    assert_eq!(Unbuffered::WARRANTY, WarrantyClass::SharedDevice);
    assert_eq!(
        Unbuffered::CAPABILITIES,
        Capabilities::NONE.with_write_through()
    );
    assert_eq!(Unbuffered::FLAGS, FILE_FLAG_NO_BUFFERING);
    const { assert!(Unbuffered::LOCAL_DEVICE_ONLY) };
    assert_eq!(
        Unbuffered::geometry(),
        Geometry::uniform(Unbuffered::DEFAULT_SECTOR_SIZE).expect("a power of two")
    );
    assert_eq!(Unbuffered::DEFAULT_SECTOR_SIZE, 4096);
}

#[test]
fn the_kernel_treats_a_cached_file_as_cached_and_an_unbuffered_one_as_unbuffered() {
    // The same unaligned seven-byte write at offset three: a cached file takes it, an unbuffered
    // one is refused by the kernel (already at the seek, which it also holds to the sector).
    // Nothing here reads a flag this crate set, so a mode that was recorded and not applied, or
    // applied and not recorded, fails.
    let unaligned = |file: &mut File| -> io::Result<()> {
        file.seek(SeekFrom::Start(3))?;
        file.write_all(b"seven b")
    };
    let (cached_file, unbuffered_file) = (TempFile::new("of-cached"), TempFile::new("of-raw"));
    let cached = OpenedFile::<Cached>::open(&cached_file.0, read_write()).expect("open cached");
    let unbuffered =
        OpenedFile::<Unbuffered>::open(&unbuffered_file.0, read_write()).expect("open unbuffered");

    unaligned(&mut io_through(&cached)).expect("a cached file takes an unaligned write");
    let refused =
        unaligned(&mut io_through(&unbuffered)).expect_err("an unbuffered file refuses it");
    assert_eq!(refused.raw_os_error(), Some(INVALID_PARAMETER));
}

#[test]
fn an_aligned_write_to_an_unbuffered_file_succeeds() {
    let temp = TempFile::new("of-aligned");
    let unbuffered = OpenedFile::<Unbuffered>::open(&temp.0, read_write()).expect("open");
    let sector = Unbuffered::DEFAULT_SECTOR_SIZE as usize;
    let buffer = AlignedBuf::from_bytes(&vec![7u8; sector], sector).expect("an aligned buffer");
    assert!(unbuffered.geometry().admits(0, sector as u64));
    assert!(
        unbuffered
            .geometry()
            .admits_buffer(buffer.as_ptr() as usize)
    );
    let mut through = io_through(&unbuffered);
    through
        .write_all(buffer.as_slice())
        .expect("a sector-aligned write to an unbuffered file");
    assert_eq!(
        fs::metadata(&temp.0).expect("metadata").len(),
        sector as u64
    );
}

#[test]
fn a_hint_sets_its_own_flag_and_nothing_else() {
    let read = FileOptions::new().read(true);
    let flags = |options: FileOptions, mode: u32| options.parameters(mode).expect("valid").flags;

    assert_eq!(flags(read, 0), 0, "no hint, no flag");
    assert_eq!(flags(read.random_access(true), 0), FILE_FLAG_RANDOM_ACCESS);
    assert_eq!(
        flags(read.sequential_scan(true), 0),
        FILE_FLAG_SEQUENTIAL_SCAN
    );
    assert_eq!(
        flags(read.random_access(true).sequential_scan(true), 0),
        FILE_FLAG_RANDOM_ACCESS | FILE_FLAG_SEQUENTIAL_SCAN,
        "both together are passed on, not refused"
    );
    assert_eq!(
        flags(read.random_access(true).random_access(false), 0),
        0,
        "a hint can be taken back"
    );
    // Added to the mode's own flags and to the one a new file carries, never in place of them.
    assert_eq!(
        flags(read.random_access(true), FILE_FLAG_NO_BUFFERING),
        FILE_FLAG_NO_BUFFERING | FILE_FLAG_RANDOM_ACCESS
    );
    assert_eq!(
        flags(
            FileOptions::new()
                .write(true)
                .create_new(true)
                .sequential_scan(true),
            0
        ),
        FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN
    );
}

/// The four ways of asking for the two hints.
const HINTS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

/// Opens a file in mode `B` with the hints, writes one aligned sector through it and reads it
/// back, which is what the kernel does with the pair, judged by the kernel and not by a flag this
/// crate recorded.
fn open_write_and_read_back<B: Buffering>(
    temp: &TempFile,
    (random, sequential): (bool, bool),
) -> io::Result<()> {
    let options = read_write()
        .random_access(random)
        .sequential_scan(sequential);
    let opened = OpenedFile::<B>::open(&temp.0, options)?;
    let sector = Unbuffered::DEFAULT_SECTOR_SIZE as usize;
    let written = AlignedBuf::from_bytes(&vec![0x5a; sector], sector).expect("an aligned buffer");
    let mut through = io_through(&opened);
    through.seek(SeekFrom::Start(0))?;
    through.write_all(written.as_slice())?;
    through.seek(SeekFrom::Start(0))?;
    let mut read = AlignedBuf::zeroed(sector, sector).expect("an aligned buffer");
    through.read_exact(read.as_mut_slice())?;
    assert_eq!(read.as_slice(), written.as_slice());
    Ok(())
}

#[test]
fn the_kernel_takes_each_hint_on_a_cached_file() {
    for hints in HINTS {
        let temp = TempFile::new("of-hint-cached");
        open_write_and_read_back::<Cached>(&temp, hints)
            .unwrap_or_else(|error| panic!("cached, hints {hints:?}: {error}"));
    }
}

// Passed to an unbuffered file and not refused (`DI-14.1`). The `CreateFileW` page says the hints
// "have no effect if the file system does not support cached I/O and FILE_FLAG_NO_BUFFERING", which
// can be read two ways, so this is the measurement: every pair opens and does aligned I/O.
#[test]
fn the_kernel_takes_each_hint_on_an_unbuffered_file() {
    for hints in HINTS {
        let temp = TempFile::new("of-hint-unbuffered");
        open_write_and_read_back::<Unbuffered>(&temp, hints)
            .unwrap_or_else(|error| panic!("unbuffered, hints {hints:?}: {error}"));
    }
}

#[test]
fn a_hint_changes_neither_the_class_nor_the_alignment_nor_what_is_offered() {
    // Judged on the target made from each file, which is what a consumer reads.
    let claims = |options: FileOptions| {
        let (unbuffered, cached) = (TempFile::new("of-claims-u"), TempFile::new("of-claims-c"));
        let u = Win32FileTarget::new(
            OpenedFile::<Unbuffered>::open(&unbuffered.0, options).expect("open unbuffered"),
        );
        let c = Win32FileTarget::new(
            OpenedFile::<Cached>::open(&cached.0, options).expect("open cached"),
        );
        (
            (u.warranty(), u.geometry(), u.capabilities()),
            (c.warranty(), c.geometry(), c.capabilities()),
        )
    };
    let plain = claims(read_write());
    for (random, sequential) in HINTS {
        let hinted = claims(
            read_write()
                .random_access(random)
                .sequential_scan(sequential),
        );
        assert_eq!(hinted, plain, "hints {random}, {sequential}");
    }
    assert_eq!(plain.0.0, WarrantyClass::SharedDevice);
    assert_eq!(plain.1.0, WarrantyClass::BestEffort);
}

#[test]
fn a_sector_size_replaces_the_default_and_must_be_a_power_of_two() {
    let temp = TempFile::new("of-sector");
    let open = || OpenedFile::<Unbuffered>::open(&temp.0, read_write()).expect("open");
    let small = open().with_sector_size(512).expect("512 is a power of two");
    assert_eq!(small.geometry(), Geometry::uniform(512).expect("valid"));
    for bad in [0, 3, 1000, 4097] {
        let error = open()
            .with_sector_size(bad)
            .expect_err("not a power of two");
        assert_eq!(error, GeometryError { alignment: bad });
    }
}

#[test]
fn a_cached_file_has_no_sector_size_to_state() {
    // `with_sector_size` exists only for `OpenedFile<Unbuffered>`; a cached file's geometry is
    // fixed, and is read back as it was given.
    let temp = TempFile::new("of-fixed");
    let cached = OpenedFile::<Cached>::open(&temp.0, read_write()).expect("open");
    assert_eq!(cached.geometry(), Geometry::UNALIGNED);
}

#[test]
fn the_options_decide_what_is_opened_and_created() {
    let temp = TempFile::new("of-options");
    let missing = temp.0.with_extension("missing");

    let error = OpenedFile::<Cached>::open(&missing, read_write()).expect_err("not created");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);

    let created = OpenedFile::<Cached>::open(&missing, read_write().create(true)).expect("create");
    assert!(missing.exists());
    drop(created);

    let error =
        OpenedFile::<Cached>::open(&missing, read_write().create_new(true)).expect_err("it exists");
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);

    fs::write(&missing, b"content").expect("fill");
    drop(OpenedFile::<Cached>::open(&missing, read_write().truncate(true)).expect("truncate"));
    assert_eq!(fs::metadata(&missing).expect("metadata").len(), 0);

    let error = OpenedFile::<Cached>::open(&missing, FileOptions::new()).expect_err("no access");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    fs::remove_file(&missing).expect("remove");
}

#[test]
fn an_unbuffered_file_can_be_created_where_none_exists_yet() {
    // The remote check looks at the directory a file would be created in, so a new path is judged
    // before anything is created, and a missing directory is the open's own NotFound.
    let temp = TempFile::new("of-new");
    let fresh = temp.0.with_extension("fresh");
    let created = OpenedFile::<Unbuffered>::open(&fresh, read_write().create_new(true))
        .expect("create unbuffered");
    assert!(fresh.exists());
    drop(created);
    fs::remove_file(&fresh).expect("remove");

    let nowhere = temp.0.join("no-such-directory").join("file");
    let error = OpenedFile::<Unbuffered>::open(&nowhere, read_write().create(true))
        .expect_err("no directory");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn clones_share_one_handle_and_lending_gives_that_handle() {
    let temp = TempFile::new("of-clone");
    let file = OpenedFile::<Cached>::open(&temp.0, read_write()).expect("open");
    let copy = file.clone();
    assert!(file.handle().same_handle(copy.handle()));
    assert_eq!(file.as_raw_handle(), copy.as_raw_handle());
    assert_eq!(file.as_raw_handle(), file.handle().as_raw_handle());
    assert!(temp.is_held());
    drop((file, copy));
    assert!(!temp.is_held(), "the last clone closes it");
}

#[test]
fn a_target_path_that_is_a_unc_share_names_a_remote_device_in_either_form() {
    for remote in [
        r"\\server\share\dir\file",
        r"\\?\UNC\server\share\dir\file",
        r"\\?\UNC\localhost\C$\file",
        r"\\server\share",
    ] {
        assert!(names_a_remote_device(Path::new(remote)), "{remote}");
    }
}

#[test]
fn a_path_on_a_local_volume_does_not() {
    for local in [
        r"C:\dir\file",
        r"\\?\C:\dir\file",
        r"\\.\C:\dir\file",
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\file",
        r"relative\path",
        r"\rooted\path",
        "",
    ] {
        assert!(!names_a_remote_device(Path::new(local)), "{local:?}");
    }
}

#[test]
fn where_a_path_lands_is_itself_when_it_exists_and_its_directory_when_it_does_not() {
    let temp = TempFile::new("of-location");
    let existing = final_location(&temp.0).expect("an existing file");
    assert_eq!(existing, fs::canonicalize(&temp.0).expect("canonical"));

    let absent = temp.0.with_extension("absent");
    let landed = final_location(&absent).expect("the directory of a new file");
    let directory = temp.0.parent().expect("a directory");
    assert_eq!(landed, fs::canonicalize(directory).expect("canonical"));

    let relative = final_location(Path::new("absent-in-the-current-directory"))
        .expect("a bare name is judged by the current directory");
    assert_eq!(
        relative,
        fs::canonicalize(".").expect("the current directory")
    );

    let error = final_location(&temp.0.join("no-such-directory").join("file"))
        .expect_err("its directory is missing too");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

/// The administrative share of this volume, `\\localhost\X$\...`, naming `local`. Reachable on an
/// account that is an administrator, as the CI runner's is; the test that uses it is ignored by
/// default for that reason, as `windows-namespace-request-sys`'s UNC test is.
fn admin_share(local: &Path) -> PathBuf {
    let text = local.to_str().expect("a UTF-8 temp path");
    // A temp directory named with the verbatim prefix still has its drive after it.
    let plain = text.strip_prefix(r"\\?\").unwrap_or(text);
    let drive = plain.chars().next().expect("a drive letter");
    PathBuf::from(format!(r"\\localhost\{drive}${}", &plain[2..]))
}

#[test]
#[ignore = "needs the administrative share \\\\localhost\\C$, which a CI runner has"]
fn an_unbuffered_file_is_refused_on_a_remote_device_and_a_cached_one_is_not() {
    let temp = TempFile::new("of-remote");
    let remote = admin_share(&temp.0);

    let error = OpenedFile::<Unbuffered>::open(&remote, read_write())
        .expect_err("an unbuffered file on a share");
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    let refusal = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<RemoteDevice>())
        .expect("the cause is a RemoteDevice");
    assert!(
        refusal.location.to_string_lossy().starts_with(r"\\?\UNC\"),
        "{:?}",
        refusal.location
    );
    assert!(refusal.to_string().contains("remote device"));

    OpenedFile::<Cached>::open(&remote, read_write())
        .expect("a cached file on a share claims only best effort, and is allowed");
}

// --- What an open asks of CreateFileW -------------------------------------------------------
//
// Opening went from `std::fs::OpenOptions` to `windows-namespace-request-sys`'s `OpenFile`, and
// a caller must see no difference, so what std asked of the kernel is pinned here.

fn parameters_of(options: FileOptions) -> open_request::Parameters {
    options.parameters(0).expect("a valid combination")
}

fn parameters(
    access: u32,
    disposition: u32,
    flags: u32,
    truncate_after: bool,
) -> open_request::Parameters {
    open_request::Parameters {
        access,
        disposition,
        flags,
        truncate_after,
    }
}

#[test]
fn the_options_map_to_the_access_and_disposition_std_asked_for() {
    assert_eq!(
        parameters_of(FileOptions::new().read(true)),
        parameters(GENERIC_READ, OPEN_EXISTING, 0, false)
    );
    assert_eq!(
        parameters_of(FileOptions::new().write(true)),
        parameters(GENERIC_WRITE, OPEN_EXISTING, 0, false)
    );
    assert_eq!(
        parameters_of(read_write()),
        parameters(GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING, 0, false)
    );
    assert_eq!(
        parameters_of(read_write().create(true)),
        parameters(GENERIC_READ | GENERIC_WRITE, OPEN_ALWAYS, 0, false)
    );
    assert_eq!(
        parameters_of(read_write().truncate(true)),
        parameters(GENERIC_READ | GENERIC_WRITE, TRUNCATE_EXISTING, 0, false)
    );
}

#[test]
fn a_created_and_truncated_file_is_opened_always_and_emptied_afterwards() {
    // Not `CREATE_ALWAYS`, whose semantics std avoids the same way (rust-lang/rust#115745).
    assert_eq!(
        parameters_of(read_write().create(true).truncate(true)),
        parameters(GENERIC_READ | GENERIC_WRITE, OPEN_ALWAYS, 0, true)
    );
}

#[test]
fn a_new_file_is_created_without_following_a_link_and_whatever_else_is_asked() {
    let expected = parameters(
        GENERIC_READ | GENERIC_WRITE,
        CREATE_NEW,
        FILE_FLAG_OPEN_REPARSE_POINT,
        false,
    );
    assert_eq!(parameters_of(read_write().create_new(true)), expected);
    // `create_new` wins over `create` and `truncate`, as it does in std.
    assert_eq!(
        parameters_of(read_write().create_new(true).create(true).truncate(true)),
        expected
    );
}

#[test]
fn the_buffering_flags_are_added_to_what_the_options_imply() {
    let flags = read_write()
        .create_new(true)
        .parameters(FILE_FLAG_NO_BUFFERING)
        .expect("valid")
        .flags;
    assert_eq!(flags, FILE_FLAG_NO_BUFFERING | FILE_FLAG_OPEN_REPARSE_POINT);
    assert_eq!(
        read_write()
            .parameters(FILE_FLAG_NO_BUFFERING)
            .expect("valid")
            .flags,
        FILE_FLAG_NO_BUFFERING
    );
}

#[test]
fn a_combination_that_cannot_mean_anything_is_invalid_input() {
    for (label, options) in [
        ("nothing", FileOptions::new()),
        ("create only", FileOptions::new().create(true)),
        ("create_new only", FileOptions::new().create_new(true)),
        ("truncate only", FileOptions::new().truncate(true)),
        (
            "read and create",
            FileOptions::new().read(true).create(true),
        ),
        (
            "read and create_new",
            FileOptions::new().read(true).create_new(true),
        ),
        (
            "read and truncate",
            FileOptions::new().read(true).truncate(true),
        ),
    ] {
        let error = options.parameters(0).expect_err(label);
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{label}");
    }
}

#[test]
fn creating_and_truncating_empties_an_existing_file_on_either_buffering_mode() {
    // The emptying is a call made after the open, so it is tried on an unbuffered handle too.
    let temp = TempFile::new("of-truncate");
    let options = read_write().create(true).truncate(true);

    fs::write(&temp.0, b"content").expect("fill");
    drop(OpenedFile::<Cached>::open(&temp.0, options).expect("cached"));
    assert_eq!(fs::metadata(&temp.0).expect("metadata").len(), 0);

    fs::write(&temp.0, b"content").expect("fill");
    drop(OpenedFile::<Unbuffered>::open(&temp.0, options).expect("unbuffered"));
    assert_eq!(fs::metadata(&temp.0).expect("metadata").len(), 0);

    // And where there is nothing to empty, the file is created.
    let fresh = temp.0.with_extension("fresh");
    drop(OpenedFile::<Cached>::open(&fresh, options).expect("created"));
    assert_eq!(fs::metadata(&fresh).expect("metadata").len(), 0);
    fs::remove_file(&fresh).expect("remove");
}

#[test]
fn an_opened_file_is_shared_for_reading_writing_and_deletion_as_std_opened_one() {
    let temp = TempFile::new("of-share");
    let held = OpenedFile::<Cached>::open(&temp.0, read_write()).expect("first");
    let second = OpenedFile::<Cached>::open(&temp.0, read_write()).expect("a second open");
    drop(second);
    // Deleting a file that is open needs the delete share, which std's default gave.
    fs::remove_file(&temp.0).expect("a file opened shared for deletion can be deleted");
    drop(held);
}
