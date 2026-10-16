// Copyright (c) 2026 Mike Grier
//! Opening volumes the way a test of this crate needs them: with no access, which an ordinary
//! account can do, so the device controls that need none run anywhere.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

/// The volume the system is installed on, as a device path.
pub(crate) fn system_volume_path() -> String {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    format!(r"\\.\{drive}")
}

/// The drive of a local path, such as `C:`, whether the path is plain (`C:\x`) or has the verbatim
/// or device prefix (`\\?\C:\x`, `\\.\C:\x`). A UNC path, plain or verbatim, has none.
fn drive_of(path: &str) -> Option<&str> {
    let plain = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\\.\"))
        .unwrap_or(path);
    let bytes = plain.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':').then(|| &plain[..2])
}

/// The volume the temporary directory is on, as a device path: not always the system's.
pub(crate) fn temp_volume_path() -> String {
    let temp = std::env::temp_dir();
    let drive = temp
        .to_str()
        .and_then(drive_of)
        .expect("a temp directory with a drive letter");
    format!(r"\\.\{drive}")
}

/// `path` opened with no access, shared with everyone; overlapped if asked.
pub(crate) fn open_volume(path: &str, overlapped: bool) -> File {
    OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(if overlapped { FILE_FLAG_OVERLAPPED } else { 0 })
        .open(path)
        .unwrap_or_else(|error| panic!("open {path} with no access: {error}"))
}

/// A file that is not a volume, for the controls to refuse.
pub(crate) fn ordinary_file(tag: &str) -> (File, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("win-volume-sys-{tag}-{}", std::process::id()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("create an ordinary file");
    (file, path)
}

/// The numbers of the disks the system volume is on, in the order the device reports them.
pub(crate) fn system_disk_numbers() -> Vec<u32> {
    crate::volume_disk_extents(&open_volume(&system_volume_path(), false))
        .expect("the system volume's disks")
        .iter()
        .map(|extent| extent.disk_number)
        .collect()
}

/// Disk `number`, opened with no access, which an ordinary account can do; overlapped if asked.
pub(crate) fn open_disk(number: u32, overlapped: bool) -> File {
    open_volume(&format!(r"\\.\PhysicalDrive{number}"), overlapped)
}

/// The first disk the system volume is on, opened with no access.
pub(crate) fn system_disk(overlapped: bool) -> File {
    open_disk(system_disk_numbers()[0], overlapped)
}

#[cfg(test)]
mod tests;
