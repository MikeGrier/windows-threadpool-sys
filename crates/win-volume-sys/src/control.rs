// Copyright (c) 2026 Mike Grier
//! The one call every control in this crate goes through.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, FALSE, GetLastError, TRUE};
use windows_sys::Win32::System::IO::{DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::CreateEventW;

/// Send `code`, which takes no input, to `handle`, and read what it answers into `output`; the
/// number of bytes the device wrote.
///
/// The call is made with an `OVERLAPPED` and an event of its own, and waited for, so it is the same
/// call whether the handle was opened overlapped or not: a device control on an overlapped handle
/// with no `OVERLAPPED` is undefined, and this crate cannot see how a handle it is lent was opened.
/// It returns only when the device has finished with `output`.
///
/// On `ERROR_MORE_DATA` the error is returned and `output` holds what fitted, which is how a
/// control that reports a count first is sized.
pub(crate) fn control(handle: &impl AsHandle, code: u32, output: &mut [u8]) -> io::Result<usize> {
    let output_len = u32::try_from(output.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "an output buffer over 4 GiB"))?;

    // SAFETY: the arguments are valid for the call -- no security attributes, an unnamed
    // manual-reset event, unsignalled to start.
    let event = unsafe { CreateEventW(ptr::null(), TRUE, FALSE, ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `event` is a handle this call just created and nothing else owns.
    let event = unsafe { OwnedHandle::from_raw_handle(event) };

    // SAFETY: all-zero is a valid `OVERLAPPED`, and its event is then set.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event.as_raw_handle();

    let device = handle.as_handle().as_raw_handle();
    let mut written = 0u32;
    // SAFETY: `device` is borrowed from `handle`, open for the call. There is no input. `output`
    // is writable for `output_len` bytes, and `overlapped` and `written` live until this function
    // returns, which it does only after the device has finished: the call completed here, or the
    // wait below has.
    let started = unsafe {
        DeviceIoControl(
            device,
            code,
            ptr::null(),
            0,
            output.as_mut_ptr().cast(),
            output_len,
            &mut written,
            &mut overlapped,
        )
    };
    if started == FALSE {
        // SAFETY: read straight after the call that set it.
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        // SAFETY: the same handle and `OVERLAPPED` the call took, waited for to completion.
        let finished = unsafe { GetOverlappedResult(device, &overlapped, &mut written, TRUE) };
        if finished == FALSE {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(written as usize)
}
