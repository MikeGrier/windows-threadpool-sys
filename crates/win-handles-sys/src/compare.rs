// Copyright (c) 2026 Mike Grier
//! Whether two handles name the same kernel object: `CompareObjectHandles`.

use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Win32::Foundation::{CompareObjectHandles, FALSE};

/// Whether `first` and `second` name the same kernel object: `CompareObjectHandles`.
///
/// The page says the call is for deciding this "without imposing a requirement that specific access rights be granted to either
/// handle in order to perform the comparison", so a handle that grants nothing can still be compared. Its example is whether a
/// process handle is the current process, which the current process's pseudo-handle answers. The comparison of two handles of
/// different types of object is "legal to make, though the function will always return FALSE".
///
/// The result is the call's `TRUE` or `FALSE` and no more: the page documents no failure, and the handles are open for the
/// borrows, so there is none to report. A `FALSE` is that the two do not name one object. Each handle is its own: two duplicates
/// of one object are the same object, and two opens of one file are not (WH-D-4).
///
/// Needs Windows 10 or Windows Server 2016, which the page gives as the first that has the call.
///
/// # Example
///
/// ```
/// use std::fs::File;
///
/// use win_handles_sys::compare_object_handles;
///
/// let path = std::env::temp_dir().join(format!("wh-compare-{}.tmp", std::process::id()));
/// let file = File::create(&path)?;
/// let duplicate = file.try_clone()?;
/// let another_open = File::open(&path)?;
///
/// assert!(compare_object_handles(&file, &duplicate));
/// // Opening a file again makes another file object, whatever the file.
/// assert!(!compare_object_handles(&file, &another_open));
/// # drop((file, duplicate, another_open));
/// # std::fs::remove_file(&path)?;
/// # Ok::<(), std::io::Error>(())
/// ```
#[must_use]
pub fn compare_object_handles(first: impl AsHandle, second: impl AsHandle) -> bool {
    // SAFETY: both handles are open for the borrows.
    let same = unsafe {
        CompareObjectHandles(
            first.as_handle().as_raw_handle(),
            second.as_handle().as_raw_handle(),
        )
    };
    same != FALSE
}

#[cfg(test)]
mod tests;
