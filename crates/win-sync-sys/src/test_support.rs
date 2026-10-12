// Copyright (c) 2026 Mike Grier
//! Helpers shared by the unit tests of the wait functions.
//!
//! What a wait did to an object is observed with a raw `WaitForSingleObject` here, never through
//! the crate's own waits, so a defect in the code under test cannot hide behind itself.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

use crate::{Event, ResetMode};

/// How long a test waits for something that should happen.
pub const BOUND: Duration = Duration::from_secs(5);

pub fn auto(signalled: bool) -> Event {
    Event::new(ResetMode::Auto, signalled).expect("an event")
}

pub fn manual(signalled: bool) -> Event {
    Event::new(ResetMode::Manual, signalled).expect("an event")
}

/// Whether `object` is signalled now, by a raw zero-timeout wait.
///
/// **This takes the signal of an auto-reset event**, as any satisfied wait does, and acquires a
/// mutex.
pub fn is_signalled(object: &impl AsHandle) -> bool {
    // SAFETY: a borrowed handle is open for the borrow's lifetime.
    match unsafe { WaitForSingleObject(object.as_handle().as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => true,
        WAIT_TIMEOUT => false,
        other => panic!("WaitForSingleObject returned {other:#x}"),
    }
}

/// A mutex whose owner has ended without releasing it.
pub fn abandoned_mutex() -> OwnedHandle {
    std::thread::spawn(|| {
        // SAFETY: an unnamed mutex with default security, initially owned by this thread, which
        // ends without releasing it.
        let raw = unsafe { CreateMutexW(std::ptr::null(), 1, std::ptr::null()) };
        assert!(
            !raw.is_null(),
            "CreateMutexW: {}",
            io::Error::last_os_error()
        );
        // SAFETY: the call returned a new handle that nothing else owns.
        unsafe { OwnedHandle::from_raw_handle(raw) }
    })
    .join()
    .expect("the owning thread")
}

/// Counts how many times it was dropped, to show a routine that was neither run nor leaked.
pub struct DropCount(pub Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
