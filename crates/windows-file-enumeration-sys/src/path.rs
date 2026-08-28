// Copyright (c) 2026 Mike Grier
//! The request path contract: what a caller may name, and what gets stored.
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

use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;
use wtf_string::{Wtf16Str, Wtf16String};

use crate::error::{RequestError, RequestFailure, Win32Error};

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

/// Validate and, where the contract calls for it, resolve a caller's path.
///
/// The returned value is the exact path a worker will later open.
///
/// # Errors
///
/// Returns [`RequestError`] for an empty path, an interior NUL, a `\\?\` path
/// that is not fully qualified, an ordinary path that exceeds `MAX_PATH` before
/// or after resolution, or a resolution failure reported by Windows.
pub(crate) fn prepare(path: &Wtf16Str) -> Result<Wtf16String, RequestError> {
    if path.is_empty() {
        return Err(RequestError::new(RequestFailure::EmptyPath));
    }
    if path.has_interior_nul() {
        return Err(RequestError::new(RequestFailure::InteriorNul));
    }

    let units = path.as_units();
    if units.starts_with(&VERBATIM_PREFIX) {
        validate_verbatim(&units[VERBATIM_PREFIX.len()..])?;
        return Ok(Wtf16String::from_units(units));
    }

    if units.len() > MAX_PATH_CONTENT {
        return Err(RequestError::new(RequestFailure::PathTooLong));
    }
    resolve(path)
}

/// Check that a `\\?\` path names an absolute root.
///
/// `rest` is everything after the prefix. Win32 will not parse this path, so it
/// must already be the absolute form: a relative or rootless verbatim path
/// cannot be repaired later and would fail at open with a code that says nothing
/// about why.
fn validate_verbatim(rest: &[u16]) -> Result<(), RequestError> {
    let not_qualified = || RequestError::new(RequestFailure::NotFullyQualified);

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
fn resolve(path: &Wtf16Str) -> Result<Wtf16String, RequestError> {
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
        let code = Win32Error::last();
        // The buffer was handed to Win32 and may hold anything; restore the
        // empty-string invariant before the value is dropped or observed.
        // SAFETY: zero content units are trivially initialised and within
        // capacity.
        unsafe { resolved.set_len_from_ffi(0) };
        return Err(RequestError::with_code(
            RequestFailure::PathResolution,
            code,
        ));
    }

    let written = written as usize;
    // A return at or above the buffer size is the "needed this much including
    // the terminator" form, which here can only mean the resolved path does not
    // fit the ordinary limit. Nothing usable was written, so the invariant is
    // restored the same way as on failure.
    if written > MAX_PATH_CONTENT {
        // SAFETY: as above.
        unsafe { resolved.set_len_from_ffi(0) };
        return Err(RequestError::new(RequestFailure::PathTooLong));
    }

    // SAFETY: `GetFullPathNameW` wrote `written` content units plus its own
    // terminator, and `written` is within the requested capacity.
    unsafe { resolved.set_len_from_ffi(written) };
    if resolved.is_empty() {
        // Defensive: a non-zero return with no content would leave a path that
        // names nothing, which must not reach a worker.
        return Err(RequestError::new(RequestFailure::PathResolution));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests;
