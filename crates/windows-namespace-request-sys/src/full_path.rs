// Copyright (c) Mike Grier.

//! The `GetFullPathNameW` entry.
//!
//! Entry 9 of the audited catalogue, and the only one that takes neither a
//! handle nor produces one.
//!
//! # What it solves, and what it leaves standing
//!
//! This call is **lexical**. It resolves relative components and `.`/`..`
//! against the process current directory, and it touches no filesystem: it will
//! happily resolve a path to something that does not exist.
//!
//! So it solves exactly one problem -- the process current directory is shared
//! mutable state that any thread can change, so a relative path means something
//! different depending on *when* it is resolved. Performing this on the
//! submitting thread pins that meaning.
//!
//! It does **not** solve the session-relative drive-letter hazard, and saying
//! so plainly matters more than the part it does solve. `GetFullPathNameW`
//! never expands a drive letter, and a drive letter is resolved against the
//! logon session of whatever token is in effect at open time. A path resolved
//! here and opened on a worker under a captured token from another logon
//! session can still name a different device. That hazard is open at the
//! workspace level; this entry inherits it and does not close it.
//!
//! A consumer that wants the *final*, filesystem-verified path of an object
//! wants [`crate::final_path`], which requires a handle and therefore an open.

use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;
use wtf_string::{Wtf16Str, Wtf16String};

use crate::outcome::{Outcome, Win32Error, perform_nonzero};

/// How many times the buffer is grown before giving up.
///
/// As in [`crate::final_path`], one retry is the expected path; more means the
/// answer is changing under us.
const MAX_ATTEMPTS: usize = 8;

/// The buffer size the first attempt uses, in characters.
const FIRST_ATTEMPT_CHARS: usize = 260;

/// An owned, marshalable parameter set for `GetFullPathNameW`.
///
/// # Example
///
/// ```
/// use windows_namespace_request_sys::full_path::ResolveFullPath;
/// use wtf_string::Wtf16String;
///
/// // Lexical: `.` and `..` are resolved without touching the filesystem.
/// let resolved = ResolveFullPath::new(Wtf16String::from(r"C:\Windows\System32\..\.\Temp"))
///     .perform()?
///     .to_string_lossy();
///
/// assert_eq!(resolved, r"C:\Windows\Temp");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Example: it does not check existence
///
/// ```
/// use windows_namespace_request_sys::full_path::ResolveFullPath;
/// use wtf_string::Wtf16String;
///
/// // A path to nothing resolves perfectly happily, because the call is
/// // lexical. A consumer wanting a verified path wants an open plus
/// // GetFinalPathNameByHandleW instead.
/// let resolved = ResolveFullPath::new(Wtf16String::from(r"C:\no-such-directory\..\file.txt"))
///     .perform()?
///     .to_string_lossy();
///
/// assert_eq!(resolved, r"C:\file.txt");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
#[must_use = "an unperformed request resolves nothing"]
pub struct ResolveFullPath {
    path: Wtf16String,
}

impl ResolveFullPath {
    /// Begins a request to resolve `path`.
    ///
    /// Takes a raw path rather than a [`crate::path::PreparedPath`], because
    /// preparation is what this call *performs*. Handing it an already-prepared
    /// path would be resolving twice.
    pub fn new(path: Wtf16String) -> Self {
        Self { path }
    }

    /// The path this request will resolve.
    #[must_use]
    pub fn path(&self) -> &Wtf16Str {
        &self.path
    }

    /// Performs the call on the calling thread, growing the buffer as needed.
    ///
    /// Resolution happens against the current directory of **whichever thread
    /// performs this**, which is the one thing a caller must keep in mind: a
    /// request built on a submitter and performed on a worker resolves against
    /// the process current directory as it stands at *performance* time.
    /// [`crate::path::prepare`] is the function for pinning that at
    /// construction.
    ///
    /// # Errors
    ///
    /// Returns the raw Win32 code, unaltered.
    pub fn perform(&self) -> Outcome<Wtf16String> {
        let mut capacity = FIRST_ATTEMPT_CHARS;

        for _ in 0..MAX_ATTEMPTS {
            let mut buffer = Wtf16String::with_capacity(capacity);
            let requested = u32::try_from(capacity).unwrap_or(u32::MAX);

            let written = perform_nonzero(|| {
                // SAFETY: the input has no interior NUL by Wtf16String's own
                // invariant for a terminated pointer, and the buffer is
                // writable for `requested` characters. The buffer's invariant
                // is restored below before it is observed.
                unsafe {
                    GetFullPathNameW(
                        self.path.as_terminated_ptr(),
                        requested,
                        buffer.as_mut_ptr(),
                        core::ptr::null_mut(),
                    )
                }
            })?;

            let written = written as usize;
            if written < capacity {
                // Success: `written` excludes the terminator.
                // SAFETY: exactly `written` content characters were written,
                // within the requested capacity.
                unsafe { buffer.set_len_from_ffi(written) };
                return Ok(buffer);
            }

            // Too small: `written` is the size required *including* the
            // terminator, and nothing usable was written.
            capacity = written;
        }

        // The required size kept changing across every attempt. Report it
        // rather than looping, for the reason final_path gives: spinning here
        // would hang the worker a consumer moved this call onto.
        //
        // `ERROR_INSUFFICIENT_BUFFER` is not a reclassification -- it is
        // accurately what happened on each attempt, and this crate has no
        // better code to offer for "and it kept happening".
        Err(Win32Error::from_code(ERROR_INSUFFICIENT_BUFFER))
    }
}

impl crate::request::Request for ResolveFullPath {
    type Error = crate::Win32Error;
    type Output = Wtf16String;

    fn perform(&self) -> Outcome<Wtf16String> {
        Self::perform(self)
    }
}

#[cfg(test)]
mod tests;
