// Copyright (c) Mike Grier.
// Copied from windows-file-enumeration-sys/src/path.rs at 126eb5f.

//! The request path contract: what a caller may name, and what gets stored.
//!
//! # Duplicated on purpose, for now
//!
//! This is a second copy of the preparation that ships in
//! `windows-file-enumeration-sys`, not a replacement for it. That crate is
//! released and this one is not, so making it depend here would make it
//! unpublishable; the copy keeps the working crate untouched while this one is
//! proven. The de-duplication happens after this branch merges with `main`, and
//! is scheduled as a checklist item -- it is not a duplicate that nobody
//! circled back to.
//!
//! # A resolved path is not a session-independent path
//!
//! `GetFullPathNameW` is **lexical**. It resolves relative components and
//! `.`/`..` and never expands a drive letter, and a drive letter resolves
//! against the *logon session* of whatever token is in effect. So a path
//! prepared on a submitting thread and opened on a worker under a captured
//! token from another session can name a different device. Preparation closes
//! the current-directory race; it does not close that one, and nothing here
//! should be read as implying otherwise.
//!
//! A request resolves its path **when it is built**, on the submitting thread.
//! Deferring that to a worker would let the meaning of a relative path change
//! between submission and execution, because the process current directory is
//! shared mutable state that nothing in this crate controls. Resolving early
//! also separates the two concerns cleanly: string resolution happens here, and
//! the privileged open happens later under the captured token.
//!
//! # Two path families
//!
//! A `\\?\` path is *verbatim*: Win32 disables path parsing for it, so the crate
//! stores it code unit for code unit. It is checked for full qualification --
//! the one property the prefix promises and a caller can get wrong -- and
//! otherwise left alone. Trailing separators and `.`/`..` components are
//! preserved, because in verbatim form they are literal name components rather
//! than syntax.
//!
//! Everything else, including `\\.\` device paths, goes through
//! `GetFullPathNameW`. Those forms *are* normalised by Win32, so resolving them
//! here produces exactly the path a later `CreateFileW` would have used.
//!
//! # Why ordinary paths stop at `MAX_PATH`
//!
//! Whether `CreateFileW` accepts a longer ordinary path depends on the host
//! executable's `longPathAware` manifest and on system policy -- neither of
//! which this crate controls, and both of which belong to whoever *embeds* it.
//! Letting them decide would make the same call succeed in one host and fail in
//! another. The crate instead draws the line itself: ordinary paths stop at
//! `MAX_PATH`, and a caller who wants a longer one says so explicitly with a
//! fully qualified `\\?\` path, which has never depended on the manifest.

use std::fmt;
use std::io;

use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;
use wtf_string::{Wtf16Str, Wtf16String};

/// `MAX_PATH`: the ordinary Win32 path limit, counting the terminator.
const MAX_PATH: usize = 260;

/// The longest ordinary path content, excluding the terminator.
const MAX_PATH_CONTENT: usize = MAX_PATH - 1;

/// The Win32 verbatim prefix, `\\?\`.
const VERBATIM_PREFIX: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];

/// The `UNC\` component that follows the verbatim prefix for a network path.
const VERBATIM_UNC: [u16; 4] = [b'U' as u16, b'N' as u16, b'C' as u16, b'\\' as u16];

const BACKSLASH: u16 = b'\\' as u16;
const COLON: u16 = b':' as u16;

/// Why a caller's path could not be prepared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PathFailure {
    /// The path had no code units. An empty path names nothing.
    EmptyPath,
    /// The path contained an interior NUL. Win32 would stop at it and open a
    /// different, shorter path than the caller named.
    InteriorNul,
    /// An ordinary path, or the fully qualified form it resolved to, did not
    /// fit the ordinary `MAX_PATH` limit including its terminator.
    ///
    /// This limit is deliberate rather than incidental: it keeps behaviour
    /// independent of the host executable's `longPathAware` manifest. Supply a
    /// fully qualified `\?\` path to name a longer one.
    PathTooLong,
    /// A `\?\` path was not fully qualified, so Win32 would not interpret it
    /// as the verbatim absolute path that prefix promises.
    NotFullyQualified,
    /// Windows could not resolve an ordinary path to its fully qualified form.
    PathResolution,
}

impl PathFailure {
    /// A short description of the failure, without any raw code.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::EmptyPath => "the path was empty",
            Self::InteriorNul => "the path contained an interior NUL",
            Self::PathTooLong => "the path exceeded MAX_PATH",
            Self::NotFullyQualified => "the verbatim path was not fully qualified",
            Self::PathResolution => "the path could not be resolved",
        }
    }
}

/// A synchronous failure while preparing a caller's path.
///
/// Preparation happens where the caller is, so this is reported at
/// construction rather than from the thread that would later have opened the
/// path.
#[derive(Debug)]
pub struct PathError {
    failure: PathFailure,
    source: Option<io::Error>,
}

impl PathError {
    fn new(failure: PathFailure) -> Self {
        Self {
            failure,
            source: None,
        }
    }

    fn with_last_os(failure: PathFailure) -> Self {
        Self {
            failure,
            source: Some(io::Error::last_os_error()),
        }
    }

    /// What about the path was rejected.
    #[must_use]
    pub fn failure(&self) -> PathFailure {
        self.failure
    }

    /// The raw Win32 code behind the failure, when Windows produced one.
    ///
    /// Only [`PathFailure::PathResolution`] arises from a Win32 call; the other
    /// failures are decided here before any call is made.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.as_ref().and_then(io::Error::raw_os_error)
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(source) => write!(f, "{}: {source}", self.failure.description()),
            None => f.write_str(self.failure.description()),
        }
    }
}

impl std::error::Error for PathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// A path that has been through [`prepare`]: the exact path a worker will open.
///
/// # This is not a session-independent path
///
/// Preparation resolves against the *process* current directory, on the
/// calling thread, which is what stops the meaning of a relative path changing
/// between submission and execution. It does **not** expand a drive letter,
/// because `GetFullPathNameW` is lexical and never does -- and a drive letter
/// is resolved against the logon session of the token in effect at open time.
/// A prepared path carried to a worker running under a captured token from
/// another logon session can therefore still name a different device. That
/// hazard is open at the workspace level; this type inherits it rather than
/// resolving it.
///
/// # Example
///
/// An ordinary path is resolved to its fully qualified form here, on the
/// calling thread, so the meaning cannot change before a worker opens it:
///
/// ```
/// use windows_namespace_request_sys::prepare;
/// use wtf_string::Wtf16String;
///
/// let prepared = prepare(&Wtf16String::from(r"C:\Windows\.\System32"))?;
///
/// // `.` is resolved away, exactly as a later CreateFileW would have done.
/// assert_eq!(prepared.as_wtf16().to_string_lossy(), r"C:\Windows\System32");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Example: a verbatim path is kept exactly
///
/// Win32 disables path parsing for a `\?\` path, so trailing separators and
/// `.` components are literal name components rather than syntax. Preparation
/// checks it is fully qualified and otherwise leaves it alone:
///
/// ```
/// use windows_namespace_request_sys::prepare;
/// use wtf_string::Wtf16String;
///
/// let verbatim = prepare(&Wtf16String::from(r"\\?\C:\Windows\"))?;
/// assert_eq!(verbatim.as_wtf16().to_string_lossy(), r"\\?\C:\Windows\");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PreparedPath {
    units: Wtf16String,
}

impl PreparedPath {
    /// The prepared path's code units.
    #[must_use]
    pub fn as_wtf16(&self) -> &Wtf16Str {
        &self.units
    }

    /// Releases the prepared path's owned string.
    #[must_use]
    pub fn into_wtf16(self) -> Wtf16String {
        self.units
    }

    /// A NUL-terminated pointer to the path, for passing to a Win32 call.
    ///
    /// Borrows from this value and must not outlive it. Preparation rejects an
    /// interior NUL, so the terminator is unambiguous.
    pub(crate) fn as_wtf16_terminated(&self) -> *const u16 {
        self.units.as_terminated_ptr()
    }
}

/// Validate and, where the contract calls for it, resolve a caller's path.
///
/// The returned value is the exact path a worker will later open, subject to
/// the session hazard [`PreparedPath`] documents.
///
/// # Errors
///
/// Returns [`PathError`] for an empty path, an interior NUL, a `\?\` path
/// that is not fully qualified, an ordinary path that exceeds `MAX_PATH` before
///
/// # Example
///
/// Each rejection names what was wrong, on the calling thread, rather than
/// producing a path that fails later with a code that explains nothing:
///
/// ```
/// use windows_namespace_request_sys::path::PathFailure;
/// use windows_namespace_request_sys::prepare;
/// use wtf_string::Wtf16String;
///
/// let empty = prepare(&Wtf16String::new()).expect_err("an empty path names nothing");
/// assert_eq!(empty.failure(), PathFailure::EmptyPath);
///
/// // A verbatim path that is not fully qualified cannot be repaired later,
/// // because Win32 will not parse it.
/// // A drive-RELATIVE verbatim path is refused: verbatim parsing would
/// // treat the whole thing as a literal name rather than the current
/// // directory on C:, and that cannot be repaired later.
/// let drive_relative = prepare(&Wtf16String::from(r"\\?\C:relative\path"))
///     .expect_err("a drive-relative verbatim path is not fully qualified");
/// assert_eq!(drive_relative.failure(), PathFailure::NotFullyQualified);
///
/// // These are decided here, before any Win32 call, so they carry no OS code.
/// assert_eq!(empty.raw_os_error(), None);
/// ```/// or after resolution, or a resolution failure reported by Windows.
pub fn prepare(path: &Wtf16Str) -> Result<PreparedPath, PathError> {
    prepare_units(path).map(|units| PreparedPath { units })
}

fn prepare_units(path: &Wtf16Str) -> Result<Wtf16String, PathError> {
    if path.is_empty() {
        return Err(PathError::new(PathFailure::EmptyPath));
    }
    if path.has_interior_nul() {
        return Err(PathError::new(PathFailure::InteriorNul));
    }

    let units = path.as_units();
    if units.starts_with(&VERBATIM_PREFIX) {
        validate_verbatim(&units[VERBATIM_PREFIX.len()..])?;
        return Ok(Wtf16String::from_units(units));
    }

    if units.len() > MAX_PATH_CONTENT {
        return Err(PathError::new(PathFailure::PathTooLong));
    }
    resolve(path)
}

/// Check that a `\\?\` path names an absolute root.
///
/// `rest` is everything after the prefix. Win32 will not parse this path, so it
/// must already be the absolute form: a relative or rootless verbatim path
/// cannot be repaired later and would fail at open with a code that says nothing
/// about why.
fn validate_verbatim(rest: &[u16]) -> Result<(), PathError> {
    let not_qualified = || PathError::new(PathFailure::NotFullyQualified);

    if rest.starts_with(&VERBATIM_UNC) {
        // `\\?\UNC\server\share`: both components must be present and non-empty,
        // because a server with no share names no filesystem to enumerate.
        let after_unc = &rest[VERBATIM_UNC.len()..];
        let Some(separator) = after_unc.iter().position(|unit| *unit == BACKSLASH) else {
            return Err(not_qualified());
        };
        let server = &after_unc[..separator];
        let share = &after_unc[separator + 1..];
        let share_len = share
            .iter()
            .position(|unit| *unit == BACKSLASH)
            .unwrap_or(share.len());
        if server.is_empty() || share_len == 0 {
            return Err(not_qualified());
        }
        return Ok(());
    }

    // Any other verbatim form needs a non-empty root component followed by a
    // separator: `\\?\C:\`, `\\?\Volume{...}\`, and so on.
    let Some(separator) = rest.iter().position(|unit| *unit == BACKSLASH) else {
        return Err(not_qualified());
    };
    let root = &rest[..separator];
    if root.is_empty() {
        return Err(not_qualified());
    }
    // A DOS drive root is spelled exactly `X:`. Rejecting `\\?\C:foo` matters
    // because that is drive-*relative*: verbatim parsing would treat the whole
    // thing as a literal name rather than the current directory on C:.
    if root.contains(&COLON) && !is_drive_designator(root) {
        return Err(not_qualified());
    }
    Ok(())
}

/// Whether `root` is exactly an ASCII drive designator such as `C:`.
fn is_drive_designator(root: &[u16]) -> bool {
    let [letter, colon] = root else {
        return false;
    };
    *colon == COLON && u8::try_from(*letter).is_ok_and(|byte| byte.is_ascii_alphabetic())
}

/// Resolve an ordinary path against the current directory, as Win32 would.
fn resolve(path: &Wtf16Str) -> Result<Wtf16String, PathError> {
    // `Wtf16Str` is a borrowed slice with no terminator, so the input is copied
    // into an owned value first: `GetFullPathNameW` takes a `PCWSTR`.
    let input = Wtf16String::from_units(path.as_units());
    let mut resolved = Wtf16String::with_capacity(MAX_PATH);
    // SAFETY: `input` has no interior NUL (checked by `prepare`) so its
    // terminated pointer is a valid C string, and `resolved` has room for
    // `MAX_PATH` content units plus the terminator this call writes. The
    // buffer is not observed through any other method until `set_len_from_ffi`
    // below restores the always-terminated invariant.
    let written = unsafe {
        GetFullPathNameW(
            input.as_terminated_ptr(),
            MAX_PATH as u32,
            resolved.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    };

    if written == 0 {
        let failure = PathError::with_last_os(PathFailure::PathResolution);
        // The buffer was handed to Win32 and may hold anything; restore the
        // empty-string invariant before the value is dropped or observed.
        // SAFETY: zero content units are trivially initialised and within
        // capacity.
        unsafe { resolved.set_len_from_ffi(0) };
        return Err(failure);
    }

    let written = written as usize;
    // A return at or above the buffer size is the "needed this much including
    // the terminator" form, which here can only mean the resolved path does not
    // fit the ordinary limit. Nothing usable was written, so the invariant is
    // restored the same way as on failure.
    if written > MAX_PATH_CONTENT {
        // SAFETY: as above.
        unsafe { resolved.set_len_from_ffi(0) };
        return Err(PathError::new(PathFailure::PathTooLong));
    }

    // SAFETY: `GetFullPathNameW` wrote `written` content units plus its own
    // terminator, and `written` is within the requested capacity.
    unsafe { resolved.set_len_from_ffi(written) };
    if resolved.is_empty() {
        // Defensive: a non-zero return with no content would leave a path that
        // names nothing, which must not reach a worker.
        return Err(PathError::new(PathFailure::PathResolution));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests;
