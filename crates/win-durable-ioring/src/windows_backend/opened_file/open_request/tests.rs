// Copyright (c) 2026 Mike Grier

use std::fs;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};

use windows_namespace_request_sys::{OpenFile, PathCaptureError};
use windows_sys::Win32::Foundation::GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS, OPEN_EXISTING,
};
use wtf_string::Wtf16String;

use super::{Parameters, open, path_error, raw_code};
use crate::dioring::tests::{TempFile, wtf};

/// What a file was always opened with before the share mode was a parameter.
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

fn read_existing() -> Parameters {
    Parameters {
        access: GENERIC_READ,
        disposition: OPEN_EXISTING,
        flags: 0,
        share: SHARE_ALL,
    }
}

fn directory_deeper_than_an_ordinary_path(under: &Path) -> PathBuf {
    let mut deep = under.to_path_buf();
    while deep.as_os_str().len() < 330 {
        deep.push("a-directory-with-a-long-enough-name-to-pass-the-limit");
    }
    deep
}

/// The verbatim form of an absolute drive path.
fn verbatim(path: &Path) -> PathBuf {
    let mut prefixed = std::ffi::OsString::from(r"\\?\");
    prefixed.push(path.as_os_str());
    PathBuf::from(prefixed)
}

#[test]
fn a_file_under_a_path_longer_than_an_ordinary_one_opens_when_the_caller_made_it_verbatim() {
    let temp = TempFile::new("or-long");
    let root = temp.0.with_extension("deep");
    let directory = directory_deeper_than_an_ordinary_path(&root);
    fs::create_dir_all(verbatim(&directory)).expect("create the deep directory");
    let file = verbatim(&directory.join("file.txt"));
    fs::write(&file, b"deep").expect("create the file");

    let handle = open(wtf(&file), read_existing()).expect("open the verbatim path");
    assert_eq!(
        fs::File::from(handle).metadata().expect("metadata").len(),
        4
    );

    fs::remove_dir_all(verbatim(&root)).expect("remove the deep tree");
}

#[test]
fn a_long_path_without_its_prefix_is_what_win32_makes_of_it_and_not_this_crates_doing() {
    let temp = TempFile::new("or-unprefixed");
    let root = temp.0.with_extension("deep");
    let directory = directory_deeper_than_an_ordinary_path(&root);
    fs::create_dir_all(verbatim(&directory)).expect("create the deep directory");
    let file = directory.join("file.txt");
    fs::write(verbatim(&file), b"deep").expect("create the file");

    // `std` hands `CreateFileW` the same bytes this open does, so what it answers is the answer
    // of the call itself, whether or not the host lets an unprefixed long path through.
    let given = open(wtf(&file), read_existing());
    let std_made = fs::File::open(&file);
    match (given, std_made) {
        (Ok(_), Ok(_)) => {}
        (Err(error), Err(_)) => assert!(error.raw_os_error().is_some(), "{error}"),
        (given, std_made) => {
            // `std` lengthens a long path itself, so it can succeed where the bytes alone do not.
            assert!(
                given.is_err() && std_made.is_ok(),
                "an open of the unprefixed path succeeded where std's did not"
            );
        }
    }

    fs::remove_dir_all(verbatim(&root)).expect("remove the deep tree");
}

#[test]
fn an_empty_path_is_refused_by_win32_with_an_os_error_and_not_by_this_crate() {
    let error = open(Wtf16String::from_os_str("".as_ref()), read_existing()).expect_err("empty");
    assert!(error.raw_os_error().is_some(), "{error}");
    assert_ne!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn an_open_that_creates_leaves_an_existing_file_as_it_was_and_makes_a_new_one_empty() {
    let temp = TempFile::new("or-always");
    fs::write(&temp.0, b"content").expect("fill");
    let read_write = GENERIC_READ | windows_sys::Win32::Foundation::GENERIC_WRITE;
    let always = Parameters {
        access: read_write,
        disposition: OPEN_ALWAYS,
        flags: 0,
        share: SHARE_ALL,
    };

    let kept: OwnedHandle = open(wtf(&temp.0), always).expect("open");
    drop(kept);
    assert_eq!(fs::metadata(&temp.0).expect("metadata").len(), 7);

    let fresh = temp.0.with_extension("fresh");
    drop(open(wtf(&fresh), always).expect("create"));
    assert_eq!(fs::metadata(&fresh).expect("metadata").len(), 0);
    fs::remove_file(&fresh).expect("remove");
}

#[test]
fn what_the_kernel_refuses_reaches_the_caller_as_its_own_error() {
    let temp = TempFile::new("or-errors");
    let missing = temp.0.with_extension("missing");
    let error = open(wtf(&missing), read_existing()).expect_err("no such file");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);

    let error = open(
        wtf(&temp.0),
        Parameters {
            disposition: CREATE_NEW,
            ..read_existing()
        },
    )
    .expect_err("it exists");
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn a_path_with_an_interior_nul_is_invalid_input() {
    let with_nul = OpenFile::new(Wtf16String::from("a\0b")).expect_err("interior NUL");
    assert!(matches!(with_nul, PathCaptureError::InteriorNul));
    assert_eq!(path_error(with_nul).kind(), io::ErrorKind::InvalidInput);

    let error = open(Wtf16String::from("a\0b"), read_existing()).expect_err("interior NUL");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_path_that_is_not_unicode_reaches_the_request_unaltered() {
    use std::os::windows::ffi::OsStringExt;

    // A lone surrogate is a legal Windows file name and no `String`: the open must not lose it.
    let temp = TempFile::new("or-lone");
    let mut name = std::ffi::OsString::from(format!("{}-", std::process::id()));
    name.push(std::ffi::OsString::from_wide(&[0x61, 0xD800, 0x62]));
    let file = temp.0.with_file_name(name);
    fs::write(&file, b"x").expect("create a file with a lone surrogate in its name");

    let opened = open(wtf(&file), read_existing());
    fs::remove_file(&file).expect("remove it");
    opened.expect("the same name opens");
}

#[test]
fn a_win32_code_is_the_same_bits_as_the_signed_one_an_io_error_holds() {
    assert_eq!(raw_code(5), 5);
    assert_eq!(raw_code(0x8000_0005), i32::MIN + 5);
    assert_eq!(raw_code(u32::MAX), -1);
}
