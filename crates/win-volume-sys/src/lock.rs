// Copyright (c) 2026 Mike Grier
//! A volume's lock: `FSCTL_LOCK_VOLUME` and `FSCTL_UNLOCK_VOLUME`.

use std::io;
use std::os::windows::io::AsHandle;

use windows_sys::Win32::System::Ioctl::{FSCTL_LOCK_VOLUME, FSCTL_UNLOCK_VOLUME};

use crate::control::control;

#[cfg(test)]
mod tests;

/// Lock `volume`: until the lock is released, this handle is the only one that can use it. A file
/// system on the volume is flushed and can write nothing more, which is what lets a raw write to a
/// mounted volume's own sectors be made.
///
/// The lock belongs to the handle. It is released by [`unlock_volume`], or when the handle closes.
///
/// `volume` needs write access to the volume, and an account that is an administrator.
///
/// # Errors
///
/// The device's own error. A volume anything else has open, or has a file open on, cannot be
/// locked, and says so with an access or sharing error; nothing is changed then.
pub fn lock_volume(volume: &impl AsHandle) -> io::Result<()> {
    control(volume, FSCTL_LOCK_VOLUME, &mut []).map(|_| ())
}

/// Release the lock `volume`'s handle holds.
///
/// # Errors
///
/// The device's own error, such as `ERROR_NOT_LOCKED` for a handle that holds no lock.
pub fn unlock_volume(volume: &impl AsHandle) -> io::Result<()> {
    control(volume, FSCTL_UNLOCK_VOLUME, &mut []).map(|_| ())
}
