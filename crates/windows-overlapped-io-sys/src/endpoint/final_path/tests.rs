// Copyright (c) 2026 Mike Grier
use std::cell::RefCell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE,
    SetLastError,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, GetShortPathNameW, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::CreateNamedPipeW;
use wtf_string::Wtf16String;

use super::fill_wide;
use crate::{PathForm, UnassociatedEndpoint, VolumeName};

const ALL_VOLUMES: [VolumeName; 4] = [
    VolumeName::Dos,
    VolumeName::Guid,
    VolumeName::Nt,
    VolumeName::Omitted,
];
const ALL_FORMS: [PathForm; 2] = [PathForm::Normalized, PathForm::Opened];

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// A stand-in for a Win32 buffer-fill that reports `paths[i]` on its `i`th call,
/// as the real one would if the file's path changed between calls. Records the
/// capacity each call was given.
fn run_fake(initial: usize, paths: &[Vec<u16>]) -> (std::io::Result<Wtf16String>, Vec<u32>) {
    let capacities = RefCell::new(Vec::new());
    let result = fill_wide(initial, |buffer, capacity| {
        let call = capacities.borrow().len();
        capacities.borrow_mut().push(capacity);
        let path = &paths[call.min(paths.len() - 1)];
        let length = u32::try_from(path.len()).expect("a short test path");
        if length < capacity {
            // SAFETY: `fill_wide` gave a buffer of `capacity` units, more than `length`.
            unsafe { std::ptr::copy_nonoverlapping(path.as_ptr(), buffer, path.len()) };
            length
        } else {
            length + 1
        }
    });
    (result, capacities.into_inner())
}

// --- fill_wide, against a stand-in ---

#[test]
fn a_path_that_fits_is_read_in_one_call() {
    let (result, capacities) = run_fake(8, &[units("abc")]);
    assert_eq!(result.expect("fits").as_units(), units("abc"));
    assert_eq!(capacities, [9]);
}

#[test]
fn a_path_filling_every_unit_but_the_terminator_fits() {
    let (result, capacities) = run_fake(8, &[units("abcdefgh")]);
    assert_eq!(result.expect("fits").as_units(), units("abcdefgh"));
    assert_eq!(capacities, [9]);
}

#[test]
fn a_path_one_unit_too_long_takes_a_second_call() {
    let (result, capacities) = run_fake(8, &[units("abcdefghi")]);
    assert_eq!(result.expect("fits").as_units(), units("abcdefghi"));
    assert_eq!(capacities, [9, 10]);
}

#[test]
fn a_long_path_is_read_at_the_size_reported() {
    let long = units(&"x".repeat(300));
    let (result, capacities) = run_fake(8, std::slice::from_ref(&long));
    assert_eq!(result.expect("fits").as_units(), long);
    assert_eq!(capacities, [9, 301]);
}

#[test]
fn a_path_growing_between_calls_is_chased_until_it_fits() {
    let paths = [
        units(&"a".repeat(20)),
        units(&"b".repeat(30)),
        units("short"),
    ];
    let (result, capacities) = run_fake(8, &paths);
    assert_eq!(result.expect("fits").as_units(), units("short"));
    assert_eq!(capacities, [9, 21, 31]);
}

#[test]
fn a_callee_reporting_the_size_it_was_given_still_moves_the_loop_on() {
    let mut calls = Vec::new();
    let result = fill_wide(8, |buffer, capacity| {
        calls.push(capacity);
        if calls.len() == 1 {
            return capacity;
        }
        // SAFETY: a buffer of `capacity` units, at least one.
        unsafe { buffer.write(u16::from(b'z')) };
        1
    });
    assert_eq!(result.expect("fits").as_units(), units("z"));
    assert_eq!(calls, [9, 10]);
}

#[test]
fn an_empty_initial_buffer_grows_to_the_path() {
    let (result, capacities) = run_fake(0, &[units("path")]);
    assert_eq!(result.expect("fits").as_units(), units("path"));
    assert_eq!(capacities, [1, 5]);
}

#[test]
fn a_failure_reports_the_last_error() {
    let result = fill_wide(8, |_, _| {
        // SAFETY: sets this thread's last-error value, nothing else.
        unsafe { SetLastError(ERROR_ACCESS_DENIED) };
        0
    });
    let error = result.expect_err("the call failed");
    assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
}

#[test]
fn a_failure_after_a_retry_reports_the_last_error() {
    let mut calls = 0;
    let result = fill_wide(8, |_, capacity| {
        calls += 1;
        if calls == 1 {
            return capacity + 5;
        }
        // SAFETY: sets this thread's last-error value, nothing else.
        unsafe { SetLastError(ERROR_ACCESS_DENIED) };
        0
    });
    assert_eq!(
        result.expect_err("the retry failed").raw_os_error(),
        Some(ERROR_ACCESS_DENIED as i32)
    );
    assert_eq!(calls, 2);
}

#[test]
fn the_units_are_kept_exactly_unpaired_surrogates_and_nuls_included() {
    let path = vec![u16::from(b'a'), 0xD800, 0, u16::from(b'b'), 0xDC00];
    let (result, _) = run_fake(8, std::slice::from_ref(&path));
    assert_eq!(result.expect("fits").as_units(), path);
}

// --- final_path, against real handles ---

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-final-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).expect("create temp dir");
    dir
}

fn open(path: impl AsRef<Path>) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path.as_ref(), true, false, 0).expect("open endpoint")
}

fn as_path(path: &Wtf16String) -> PathBuf {
    PathBuf::from(path.to_os_string())
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).expect("canonicalize")
}

#[test]
fn a_file_reports_its_canonical_path() {
    let dir = temp_dir("plain");
    let path = dir.join("plain.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let reported = endpoint.observer().final_path().expect("final_path");
    assert_eq!(as_path(&reported), canonical(&path));
    drop(endpoint);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_while_the_observer_holds_the_file_is_reported() {
    let dir = temp_dir("rename");
    let first = dir.join("first.txt");
    std::fs::write(&first, b"x").expect("write");
    let endpoint = open(&first);
    let observer = endpoint.observer();
    let before = as_path(&observer.final_path().expect("before"));

    let mut current = first;
    for name in ["second.txt", "third name.txt", "\u{e9}\u{4e2d}.txt"] {
        let next = dir.join(name);
        std::fs::rename(&current, &next).expect("rename");
        let reported = as_path(&observer.final_path().expect("after rename"));
        assert_eq!(reported, canonical(&next));
        assert_ne!(reported, before);
        current = next;
    }

    drop((observer, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_move_to_another_directory_is_reported() {
    let dir = temp_dir("move");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("create subdir");
    let path = dir.join("moving.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let observer = endpoint.observer();

    let moved = elsewhere.join("moving.txt");
    std::fs::rename(&path, &moved).expect("move");
    let reported = as_path(&observer.final_path().expect("final_path"));
    assert_eq!(reported, canonical(&moved));
    assert_eq!(reported.parent(), Some(canonical(&elsewhere).as_path()));

    drop((observer, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_reports_its_path() {
    let dir = temp_dir("dir");
    let endpoint = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory");
    let reported = endpoint.observer().final_path().expect("final_path");
    assert_eq!(as_path(&reported), canonical(&dir));
    drop(endpoint);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Measured: each volume form puts its own volume name in front of the same path
/// from the volume's root.
#[test]
fn every_volume_form_names_the_volume_before_the_same_path() {
    let dir = temp_dir("volumes");
    let path = dir.join("volumes.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    let path_of = |volume| {
        observer
            .final_path_as(volume, PathForm::Normalized)
            .expect("final_path_as")
            .to_string_lossy()
    };

    let from_root = path_of(VolumeName::Omitted);
    assert!(from_root.starts_with('\\'), "{from_root}");
    for (volume, prefix) in [
        (VolumeName::Dos, r"\\?\"),
        (VolumeName::Guid, r"\\?\Volume{"),
        (VolumeName::Nt, r"\Device\"),
    ] {
        let named = path_of(volume);
        assert!(named.starts_with(prefix), "{volume:?}: {named}");
        assert!(named.ends_with(&from_root), "{volume:?}: {named}");
        assert!(named.len() > from_root.len(), "{volume:?}: {named}");
    }

    drop((observer, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Measured: an open by a differently-cased name reports the name as stored, in
/// both forms.
#[test]
fn an_open_by_another_case_reports_the_stored_case() {
    let dir = temp_dir("case");
    let path = dir.join("MixedCase.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(dir.join("MIXEDCASE.TXT"));
    let observer = endpoint.observer();
    for form in ALL_FORMS {
        let reported = as_path(
            &observer
                .final_path_as(VolumeName::Dos, form)
                .expect("final_path_as"),
        );
        assert_eq!(
            reported.file_name(),
            Some(OsString::from("MixedCase.txt").as_os_str()),
            "{form:?}"
        );
    }
    drop((observer, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Measured: an open through a hard link reports the link's own path.
#[test]
fn a_hard_link_reports_the_name_it_was_opened_by() {
    let dir = temp_dir("link");
    let original = dir.join("original.txt");
    let link = dir.join("link.txt");
    std::fs::write(&original, b"x").expect("write");
    std::fs::hard_link(&original, &link).expect("hard link");
    let by_original = open(&original);
    let by_link = open(&link);
    for form in ALL_FORMS {
        let path_of = |endpoint: &UnassociatedEndpoint| {
            as_path(
                &endpoint
                    .observer()
                    .final_path_as(VolumeName::Dos, form)
                    .expect("final_path_as"),
            )
        };
        assert_eq!(path_of(&by_original), canonical(&original), "{form:?}");
        assert_eq!(path_of(&by_link), canonical(&link), "{form:?}");
    }
    drop((by_original, by_link));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The 8.3 short name of `path`, or `None` when the volume gives it none.
fn short_name(path: &Path) -> Option<OsString> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buffer = [0_u16; 1024];
    let size = u32::try_from(buffer.len()).expect("a small buffer");
    // SAFETY: a terminated input path and an output buffer of exactly the size passed.
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), size) };
    assert!(
        written != 0 && written < size,
        "GetShortPathNameW: {}",
        std::io::Error::last_os_error()
    );
    let short = PathBuf::from(OsString::from(String::from_utf16_lossy(
        &buffer[..written as usize],
    )));
    let short_name = short.file_name()?.to_os_string();
    (Some(short_name.as_os_str()) != path.file_name()).then_some(short_name)
}

/// Measured where the volume has short names: an open by one is reported as
/// opened by `Opened` and expanded by `Normalized`. Where it has none, the two
/// forms agree.
#[test]
fn an_open_by_short_name_is_kept_as_opened_and_expanded_when_normalized() {
    let dir = temp_dir("short");
    let path = dir.join("a-long-file-name.txt");
    std::fs::write(&path, b"x").expect("write");
    let (opened_name, endpoint) = match short_name(&path) {
        Some(short) => (short.clone(), open(dir.join(short))),
        None => (OsString::from("a-long-file-name.txt"), open(&path)),
    };
    let observer = endpoint.observer();
    let name_in = |form| {
        as_path(
            &observer
                .final_path_as(VolumeName::Dos, form)
                .expect("final_path_as"),
        )
        .file_name()
        .expect("a file name")
        .to_os_string()
    };
    assert_eq!(name_in(PathForm::Opened), opened_name);
    assert_eq!(
        name_in(PathForm::Normalized),
        OsString::from("a-long-file-name.txt")
    );
    drop((observer, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_path_longer_than_max_path_is_read_whole() {
    let dir = temp_dir("long");
    // Verbatim (`\\?\`), so the open needs no long-path support from the system.
    let mut deep = canonical(&dir);
    for segment in 0..12 {
        deep.push(format!("segment-number-{segment:02}-padding"));
    }
    std::fs::create_dir_all(&deep).expect("create deep dirs");
    let path = deep.join("deep.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let reported = endpoint.observer().final_path().expect("final_path");
    assert!(reported.len() > 260, "{}", reported.len());
    assert_eq!(as_path(&reported), path);
    drop(endpoint);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_observer_of_a_handle_reads_the_same_path() {
    let dir = temp_dir("shared");
    let path = dir.join("shared.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let first = endpoint.observer();
    let second = first.clone();
    for volume in ALL_VOLUMES {
        for form in ALL_FORMS {
            assert_eq!(
                first.final_path_as(volume, form).expect("first"),
                second.final_path_as(volume, form).expect("second"),
                "{volume:?}/{form:?}"
            );
        }
    }
    drop((first, second, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Measured: the NUL device has no path in any form.
#[test]
fn the_nul_device_has_no_path() {
    let nul = UnassociatedEndpoint::open(r"\\.\NUL", true, false, 0).expect("open NUL");
    let observer = nul.observer();
    for volume in ALL_VOLUMES {
        for form in ALL_FORMS {
            let error = observer
                .final_path_as(volume, form)
                .expect_err("the NUL device has no path");
            assert_eq!(
                error.raw_os_error(),
                Some(ERROR_INVALID_PARAMETER as i32),
                "{volume:?}/{form:?}: {error:?}"
            );
        }
    }
}

/// Measured: a named pipe has no normalized path, but reports the name it was
/// opened by under its NT device, or without one.
#[test]
fn a_named_pipe_reports_only_the_name_it_was_opened_by() {
    let name = format!("windows-overlapped-io-sys-final-{}", std::process::id());
    let wide: Vec<u16> = format!(r"\\.\pipe\{name}")
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: a terminated name and no security attributes.
    let raw = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            0,
            1,
            0,
            0,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(
        raw,
        INVALID_HANDLE_VALUE,
        "{}",
        std::io::Error::last_os_error()
    );
    // SAFETY: a fresh handle this test owns, opened with FILE_FLAG_OVERLAPPED.
    let endpoint =
        unsafe { UnassociatedEndpoint::assume_overlapped(OwnedHandle::from_raw_handle(raw)) };
    let observer = endpoint.observer();

    let error = observer
        .final_path_as(VolumeName::Dos, PathForm::Normalized)
        .expect_err("a pipe has no normalized path");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_INVALID_FUNCTION as i32),
        "{error:?}"
    );
    let nt = observer
        .final_path_as(VolumeName::Nt, PathForm::Opened)
        .expect("the NT form");
    assert_eq!(nt.to_string_lossy(), format!(r"\Device\NamedPipe\{name}"));
    let bare = observer
        .final_path_as(VolumeName::Omitted, PathForm::Opened)
        .expect("without a volume");
    assert_eq!(bare.to_string_lossy(), format!(r"\{name}"));
}
