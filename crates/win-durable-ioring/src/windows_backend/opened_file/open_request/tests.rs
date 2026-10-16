// Copyright (c) 2026 Mike Grier

use std::fs;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};

use windows_namespace_request_sys::{OpenFile, PathCaptureError};
use windows_sys::Win32::Foundation::GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, OPEN_ALWAYS, OPEN_EXISTING};
use wtf_string::Wtf16String;

use super::{Parameters, open, path_error, raw_code, verbatim, verbatim_if_long};
use crate::dioring::tests::TempFile;

fn read_existing() -> Parameters {
    Parameters {
        access: GENERIC_READ,
        disposition: OPEN_EXISTING,
        flags: 0,
        truncate_after: false,
    }
}

fn directory_deeper_than_an_ordinary_path(under: &Path) -> PathBuf {
    let mut deep = under.to_path_buf();
    while deep.as_os_str().len() < 330 {
        deep.push("a-directory-with-a-long-enough-name-to-pass-the-limit");
    }
    deep
}

#[test]
fn a_short_path_is_handed_over_as_it_was_given() {
    let short = Path::new(r"C:\folder\file.txt");
    assert_eq!(verbatim_if_long(short).expect("short"), short);
    let relative = Path::new(r"folder\file.txt");
    assert_eq!(verbatim_if_long(relative).expect("relative"), relative);
}

#[test]
fn a_long_path_is_made_verbatim_and_a_short_one_never_is() {
    let long = PathBuf::from(format!(r"C:\{}\file.txt", "x".repeat(300)));
    let converted = verbatim_if_long(&long).expect("long");
    assert_eq!(
        converted,
        PathBuf::from(format!(r"\\?\C:\{}\file.txt", "x".repeat(300)))
    );
}

#[test]
fn a_relative_path_that_resolves_long_is_made_verbatim() {
    // Short as given, long once rooted: the resolved form is what the limit is judged against.
    let cwd = std::env::current_dir().expect("a current directory");
    let room = 259usize.saturating_sub(cwd.as_os_str().len());
    if room < 4 {
        eprintln!("skipped: the current directory leaves no room under the limit");
        return;
    }
    let relative = PathBuf::from("y".repeat(room));
    assert!(relative.as_os_str().len() <= 259);
    let converted = verbatim_if_long(&relative).expect("resolved long");
    assert!(
        converted.to_string_lossy().starts_with(r"\\?\"),
        "{} was not made verbatim",
        converted.display()
    );
}

#[test]
fn verbatim_prefixes_a_drive_path_and_a_unc_path_and_leaves_the_rest_alone() {
    assert_eq!(verbatim(Path::new(r"C:\a\b")), PathBuf::from(r"\\?\C:\a\b"));
    assert_eq!(
        verbatim(Path::new(r"\\server\share\a")),
        PathBuf::from(r"\\?\UNC\server\share\a")
    );
    assert_eq!(verbatim(Path::new(r"\\?\C:\a")), PathBuf::from(r"\\?\C:\a"));
    assert_eq!(
        verbatim(Path::new(r"\\?\UNC\server\share")),
        PathBuf::from(r"\\?\UNC\server\share")
    );
    assert_eq!(verbatim(Path::new(r"\\.\C:")), PathBuf::from(r"\\.\C:"));
}

#[test]
fn a_file_under_a_path_longer_than_an_ordinary_one_opens() {
    let temp = TempFile::new("or-long");
    let root = temp.0.with_extension("deep");
    let directory = directory_deeper_than_an_ordinary_path(&root);
    fs::create_dir_all(&directory).expect("create the deep directory");
    let file = directory.join("file.txt");
    fs::write(&file, b"deep").expect("create the file");

    let handle = open(&file, read_existing()).expect("open through the verbatim form");
    assert_eq!(
        fs::File::from(handle).metadata().expect("metadata").len(),
        4
    );

    fs::remove_dir_all(&root).expect("remove the deep tree");
}

#[test]
fn a_created_and_truncated_file_is_emptied_and_a_created_one_is_not() {
    let temp = TempFile::new("or-truncate");
    fs::write(&temp.0, b"content").expect("fill");
    let read_write = GENERIC_READ | windows_sys::Win32::Foundation::GENERIC_WRITE;

    let kept: OwnedHandle = open(
        &temp.0,
        Parameters {
            access: read_write,
            disposition: OPEN_ALWAYS,
            flags: 0,
            truncate_after: false,
        },
    )
    .expect("create");
    drop(kept);
    assert_eq!(fs::metadata(&temp.0).expect("metadata").len(), 7);

    let emptied = open(
        &temp.0,
        Parameters {
            access: read_write,
            disposition: OPEN_ALWAYS,
            flags: 0,
            truncate_after: true,
        },
    )
    .expect("create and truncate");
    drop(emptied);
    assert_eq!(fs::metadata(&temp.0).expect("metadata").len(), 0);
}

#[test]
fn what_the_kernel_refuses_reaches_the_caller_as_its_own_error() {
    let temp = TempFile::new("or-errors");
    let missing = temp.0.with_extension("missing");
    let error = open(&missing, read_existing()).expect_err("no such file");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);

    let error = open(
        &temp.0,
        Parameters {
            disposition: CREATE_NEW,
            ..read_existing()
        },
    )
    .expect_err("it exists");
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn a_path_that_cannot_name_a_file_is_invalid_input() {
    let error = open(Path::new(""), read_existing()).expect_err("empty");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

    let with_nul = OpenFile::new(Wtf16String::from("a\0b")).expect_err("interior NUL");
    assert!(matches!(with_nul, PathCaptureError::InteriorNul));
    assert_eq!(path_error(with_nul).kind(), io::ErrorKind::InvalidInput);

    let error = open(Path::new("a\0b"), read_existing()).expect_err("interior NUL");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_win32_code_is_the_same_bits_as_the_signed_one_an_io_error_holds() {
    assert_eq!(raw_code(5), 5);
    assert_eq!(raw_code(0x8000_0005), i32::MIN + 5);
    assert_eq!(raw_code(u32::MAX), -1);
}
