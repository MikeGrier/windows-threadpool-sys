// Copyright (c) 2026 Mike Grier
//! The observer's final-path query, behind the `fs` feature.

use std::io;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::MAX_PATH;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NAME_NORMALIZED, FILE_NAME_OPENED, GETFINALPATHNAMEBYHANDLE_FLAGS,
    GetFinalPathNameByHandleW, VOLUME_NAME_DOS, VOLUME_NAME_GUID, VOLUME_NAME_NONE, VOLUME_NAME_NT,
};
use wtf_string::Wtf16String;

use super::EndpointObserver;

#[cfg(test)]
mod tests;

/// How [`EndpointObserver::final_path_as`] names the volume a path is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VolumeName {
    /// By its drive letter (`VOLUME_NAME_DOS`).
    Dos,
    /// By its volume GUID path (`VOLUME_NAME_GUID`).
    Guid,
    /// By its NT device name (`VOLUME_NAME_NT`).
    Nt,
    /// Not at all: the path from the volume's root (`VOLUME_NAME_NONE`).
    Omitted,
}

impl VolumeName {
    const fn flag(self) -> GETFINALPATHNAMEBYHANDLE_FLAGS {
        match self {
            Self::Dos => VOLUME_NAME_DOS,
            Self::Guid => VOLUME_NAME_GUID,
            Self::Nt => VOLUME_NAME_NT,
            Self::Omitted => VOLUME_NAME_NONE,
        }
    }
}

/// Which name [`EndpointObserver::final_path_as`] reports for the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathForm {
    /// The name the file system normalizes it to (`FILE_NAME_NORMALIZED`).
    Normalized,
    /// The name the handle was opened by (`FILE_NAME_OPENED`). Measured on NTFS:
    /// an open by an 8.3 short name keeps the short name here, which
    /// [`Normalized`](Self::Normalized) expands; an open by another case or
    /// through a hard link reported the same as `Normalized`.
    Opened,
}

impl PathForm {
    const fn flag(self) -> GETFINALPATHNAMEBYHANDLE_FLAGS {
        match self {
            Self::Normalized => FILE_NAME_NORMALIZED,
            Self::Opened => FILE_NAME_OPENED,
        }
    }
}

impl EndpointObserver {
    /// The file's current path, normalized and with its volume named by drive
    /// letter: [`final_path_as`](Self::final_path_as) with [`VolumeName::Dos`]
    /// and [`PathForm::Normalized`].
    ///
    /// # Errors
    ///
    /// As [`final_path_as`](Self::final_path_as).
    pub fn final_path(&self) -> io::Result<Wtf16String> {
        self.final_path_as(VolumeName::Dos, PathForm::Normalized)
    }

    /// The file's current path, read through the handle with
    /// `GetFinalPathNameByHandleW`, as Windows' own UTF-16 unconverted.
    ///
    /// The path is the file's as of the query, so it follows renames made while
    /// the handle is open; a handle opened through a hard link reports that
    /// link's path. Synchronous and issues no overlapped I/O, so it is safe while
    /// the endpoint has operations in flight.
    ///
    /// # Errors
    ///
    /// Returns the error `GetFinalPathNameByHandleW` reports, for a handle whose
    /// device has no path in the form asked for. Measured: the NUL device refuses
    /// every form with `ERROR_INVALID_PARAMETER`; a named pipe refuses
    /// [`PathForm::Normalized`] and answers [`PathForm::Opened`] only with
    /// [`VolumeName::Nt`] (`\Device\NamedPipe\<name>`) or
    /// [`VolumeName::Omitted`] (`\<name>`).
    pub fn final_path_as(&self, volume: VolumeName, form: PathForm) -> io::Result<Wtf16String> {
        let handle = self.handle.as_raw_handle();
        let flags = volume.flag() | form.flag();
        fill_wide(MAX_PATH as usize, |buffer, capacity| {
            // SAFETY: a live handle this observer holds, and a buffer of `capacity`
            // units that `fill_wide` keeps alive and unaliased for the call.
            unsafe { GetFinalPathNameByHandleW(handle, buffer, capacity, flags) }
        })
    }
}

/// Run a Win32 buffer-fill, growing the buffer until the result fits.
///
/// `call` is given a buffer and its size in units, terminator included, and
/// returns the content length on success, the size it needs (terminator
/// included) when the buffer is too small, or 0 having set the last error. It is
/// called again until it fits, since what it reports can grow between calls.
fn fill_wide(
    mut content: usize,
    mut call: impl FnMut(*mut u16, u32) -> u32,
) -> io::Result<Wtf16String> {
    loop {
        let mut out = Wtf16String::with_capacity(content);
        let capacity = u32::try_from(content + 1).expect("a size the call itself reported");
        let written = call(out.as_mut_ptr(), capacity);
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        if written < capacity {
            // SAFETY: the call wrote `written` content units, fewer than the
            // `content + 1` the buffer was reserved for.
            unsafe { out.set_len_from_ffi(written as usize) };
            return Ok(out);
        }
        // At least one more than before, so a callee reporting the size it was
        // given cannot hold the loop in place.
        content = (written as usize - 1).max(content + 1);
    }
}
