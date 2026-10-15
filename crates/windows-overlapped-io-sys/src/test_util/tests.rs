// Copyright (c) 2026 Mike Grier
use std::collections::HashSet;

use super::{OBSERVER_QUERIES, run_every_query};
use crate::UnassociatedEndpoint;

#[test]
fn every_query_has_a_distinct_name() {
    let names: HashSet<_> = OBSERVER_QUERIES.iter().map(|query| query.name).collect();
    assert_eq!(names.len(), OBSERVER_QUERIES.len());
}

/// Measured (M17.6): on a local file every query answers but the remote one.
#[test]
fn every_query_answers_on_a_local_file_but_the_remote_one() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-test-util-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"x").expect("write temp file");
    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    let outcomes = run_every_query(&endpoint.observer());

    assert_eq!(outcomes.len(), OBSERVER_QUERIES.len());
    for (name, outcome) in &outcomes {
        assert_eq!(
            outcome.is_ok(),
            *name != "remote_protocol_info",
            "{name}: {outcome:?}"
        );
    }

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// Measured (M17.8): a named pipe has a path only as opened, under its NT name or
/// no volume -- the one place the `final_path_as` rows answer differently, so it
/// checks each row passes its own arguments.
#[test]
fn the_final_path_rows_answer_as_their_arguments_ask_on_a_pipe() {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

    let wide: Vec<u16> = format!(
        r"\\.\pipe\windows-overlapped-io-sys-test-util-{}",
        std::process::id()
    )
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

    let answered: Vec<_> = run_every_query(&endpoint.observer())
        .into_iter()
        .filter(|(name, outcome)| name.starts_with("final_path") && outcome.is_ok())
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        answered,
        [
            "final_path_as(Nt, Opened)",
            "final_path_as(Omitted, Opened)"
        ]
    );
}
