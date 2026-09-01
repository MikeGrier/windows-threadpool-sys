// Copyright (c) 2026 Mike Grier
//! Integration test: characterise the Windows behaviour that makes reopening a
//! watched directory by file reference (`OpenFileById`) useless to this crate.
//!
//! This test asserts a property of the operating system, not of this crate. It
//! exists because [D-80] rests on that property: the reopen-by-id fast path was
//! removed on the strength of it, and a design note asserting an OS limitation
//! with nothing executing it can only rot. If a future Windows accepts the read
//! below, this test fails and D-80 should be revisited.
//!
//! The mechanism, measured with a control rather than reasoned about: a handle
//! from `OpenFileById` is indistinguishable from a `CreateFileW` one by
//! synchronous/asynchronous mode, by granted access, and by the name the kernel
//! resolves for it -- yet `ReadDirectoryChangesW` rejects it with
//! `ERROR_INVALID_PARAMETER`. The only variable that changes the outcome is
//! whether the object was resolved **by file ID** or **by name**.
//!
//! [D-80]: the "Reopening by file reference" section of `DESIGN-NOTES.md`.
#![cfg(windows)]

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdType, GetFileInformationByHandle,
    OPEN_EXISTING, OpenFileById, ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIo, OVERLAPPED};

/// Closes its handle on drop, so a failing assertion cannot leak one into the
/// rest of the suite.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a live handle this type exclusively owns.
        unsafe { CloseHandle(self.0) };
    }
}

fn wide_z(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Open a directory exactly as `DirectoryHandle::open` does.
fn open_by_path(dir: &Path) -> Owned {
    let wide = wide_z(dir);
    // SAFETY: `wide` is NUL-terminated and outlives the call; the security
    // attributes and template handle are null by design.
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    };
    assert!(
        raw != INVALID_HANDLE_VALUE,
        "CreateFileW on a directory this test just created must succeed, got {}",
        // SAFETY: called immediately after the failing call above.
        unsafe { GetLastError() }
    );
    Owned(raw)
}

/// The NTFS file reference of an open handle.
fn file_reference(handle: HANDLE) -> u64 {
    // SAFETY: `handle` is live, and `info` is a valid out-parameter the callee
    // only writes.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    assert!(
        // SAFETY: as above.
        unsafe { GetFileInformationByHandle(handle, &mut info) } != 0,
        "GetFileInformationByHandle on a live directory handle must succeed"
    );
    (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
}

/// Reopen by file reference, with the same access and flags the path-based open
/// above uses.
fn reopen_by_id(volume_hint: HANDLE, file_id: u64) -> Owned {
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: u32::try_from(size_of::<FILE_ID_DESCRIPTOR>()).expect("a small fixed struct"),
        Type: FileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            FileId: file_id.cast_signed(),
        },
    };
    // SAFETY: `volume_hint` is live for the call, and `descriptor` is a fully
    // initialised `FILE_ID_DESCRIPTOR` the callee only reads.
    let raw = unsafe {
        OpenFileById(
            volume_hint,
            &descriptor,
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
        )
    };
    assert!(
        raw != INVALID_HANDLE_VALUE,
        "OpenFileById against a live handle's own file reference must succeed, got {}",
        // SAFETY: called immediately after the failing call above.
        unsafe { GetLastError() }
    );
    Owned(raw)
}

/// Issue the exact read `watcher.rs` issues, and report whether Windows accepted
/// it. A pending read is cancelled before returning, so nothing is left armed.
fn read_directory_changes_accepted(handle: HANDLE) -> Result<(), u32> {
    // `u32`-typed so the buffer is DWORD-aligned, which the API requires.
    let mut buffer = vec![0u32; 1024];
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    // SAFETY: issues one overlapped read into this test's own buffer, which the
    // kernel owns until the operation is cancelled below. `lpBytesReturned` is
    // null, which the SDK requires for an asynchronous call, and no completion
    // routine is used.
    let ok = unsafe {
        ReadDirectoryChangesW(
            handle,
            buffer.as_mut_ptr().cast(),
            u32::try_from(buffer.len() * size_of::<u32>()).expect("a fixed small buffer"),
            0,
            FILE_NOTIFY_CHANGE_FILE_NAME,
            ptr::null_mut(),
            &mut overlapped,
            None,
        )
    };
    if ok == 0 {
        // SAFETY: called immediately after the failing call above.
        return Err(unsafe { GetLastError() });
    }
    // SAFETY: `handle` is live, and cancelling is what keeps the (kernel-owned)
    // buffer from outliving this frame.
    unsafe { CancelIo(handle) };
    Ok(())
}

#[test]
fn a_directory_reopened_by_file_id_cannot_be_watched() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "windows-file-watcher-reopen-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).expect("create temp dir");

    let original = open_by_path(&dir);
    let reopened = reopen_by_id(original.0, file_reference(original.0));

    // The control. Without it, a failure below could just as easily mean this
    // test builds the read wrongly -- which is exactly the reading the original
    // investigation had to rule out.
    assert_eq!(
        read_directory_changes_accepted(original.0),
        Ok(()),
        "the control must pass: a path-opened directory handle accepts this very \
         read, so a rejection below is about how the handle was obtained and not \
         about how the read is built"
    );

    // The property D-80 rests on. Both handles requested identical access and
    // identical flags, and are indistinguishable by mode, granted access, and
    // the name the kernel resolves for them; only the *resolution* differs.
    assert_eq!(
        read_directory_changes_accepted(reopened.0),
        Err(ERROR_INVALID_PARAMETER),
        "Windows must still reject a directory-change read on a by-id open. If \
         this now succeeds, the OS limitation D-80 removed the reopen-by-id fast \
         path over no longer holds, and that decision should be revisited"
    );

    drop(reopened);
    drop(original);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
