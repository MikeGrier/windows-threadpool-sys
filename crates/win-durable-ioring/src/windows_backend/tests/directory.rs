// Copyright (c) 2026 Mike Grier
//! A directory as a target, through the ring (DI-15.5, DI-D-72): what the backend reports for each
//! operation on a directory reached by an open by identity, registered at construction and attached
//! later. An open by identity reaches a directory without `FILE_FLAG_BACKUP_SEMANTICS` (DI-D-70), and
//! the engineer ruled that it is allowed and its behaviour documented, not refused. What is asserted
//! is what was observed on the machine this was written on; the pages do not describe I/O on a
//! directory handle.

use std::fs;
use std::os::windows::fs::OpenOptionsExt;

use win_shared_os_owned_handle::SharedHandle;
use windows_sys::Win32::Foundation::ERROR_INVALID_FUNCTION;
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

use super::{Backend, GIVEN, commit, read, run, setup, write};
use crate::dioring::tests::TempFile;
use crate::error_code::error_code_from_win32;
use crate::windows_backend::{ByIdOptions, Cached, FileId, OpenedFile, Win32FileTarget};
use durable_ioring::IoTargetKey;
use durable_ioring::backend::IoBackend;

const ATTACHED: IoTargetKey = IoTargetKey(2);

#[test]
fn a_directory_target_faults_every_read_and_write_and_completes_every_commit() {
    let temp = TempFile::new("wb-directory");
    let directory = temp.0.with_extension("dir");
    fs::create_dir(&directory).expect("create the directory");
    fs::write(directory.join("child.txt"), b"x").expect("a child");
    let id = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&directory)
        .map(SharedHandle::from)
        .and_then(|handle| FileId::of(&handle))
        .expect("the directory's identity");
    let open = || {
        OpenedFile::<Cached, _>::open_by_id(
            temp.open().handle().clone(),
            id,
            ByIdOptions::new().read().write(),
        )
        .expect("the directory opens by identity")
    };

    let mut backend = Backend::new(setup(
        vec![(GIVEN, Win32FileTarget::new(open()))],
        Vec::new(),
    ))
    .expect("a directory registers with the ring");
    backend
        .attach(ATTACHED, Win32FileTarget::new(open()))
        .expect("and attaches");

    let invalid_function = Some(error_code_from_win32(ERROR_INVALID_FUNCTION));
    for key in [GIVEN, ATTACHED] {
        let written = run(&mut backend, write(1, key, 0, b"abc"));
        let fault = written.result.expect_err("a write to a directory faults");
        assert_eq!(fault.code, invalid_function, "{key:?} write: {fault:?}");

        let back = run(&mut backend, read(2, key, 0, 16));
        let fault = back.result.expect_err("a read from a directory faults");
        assert_eq!(fault.code, invalid_function, "{key:?} read: {fault:?}");

        let committed = run(&mut backend, commit(3, key));
        assert_eq!(
            committed.result.map_err(|fault| fault.code),
            Ok(0),
            "{key:?}: a commit of a directory was observed to complete"
        );
    }

    drop(backend);
    let children: Vec<_> = fs::read_dir(&directory)
        .expect("list")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert_eq!(children, ["child.txt"], "the writes changed nothing");
    fs::remove_dir_all(&directory).expect("remove the directory");
}
