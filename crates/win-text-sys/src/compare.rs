// Copyright (c) 2026 Mike Grier
//! `CompareStringOrdinal`: the ordinal comparison of two counted wide strings.
//!
//! Read 2026-10-20 from the call's page (`ms.date` 2018-12-05, `updated_at` 2024-02-22). The call takes each
//! string as a pointer and an `int` count, and this crate always passes the count, never the `-1` the
//! page gives for a NUL-terminated string, so a NUL in the data is a unit like any other. The one argument
//! the crate cannot pass as given is a slice longer than the `int` holds ([`CompareStringOrdinalError::TooLong`]).

use std::cmp::Ordering;
use std::fmt;

use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::Globalization::{
    CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringOrdinal,
};

#[cfg(test)]
mod tests;

/// Whether the comparison folds case, which is the call's `bIgnoreCase`.
///
/// The page: "TRUE if the function is to perform a case-insensitive comparison, using the operating
/// system uppercase table information", and "FALSE if the function is to compare the strings exactly as
/// they are passed in". The page also says 1 is the only numeric value that is a true value that does not
/// give an invalid parameter error, so [`Insensitive`](Self::Insensitive) is passed as `TRUE`, which is 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Case {
    /// Compare the strings exactly as they are passed in.
    Sensitive,
    /// Compare using the operating system's uppercase table.
    Insensitive,
}

impl Case {
    /// The value of `bIgnoreCase`.
    const fn ignore_case(self) -> i32 {
        match self {
            Self::Sensitive => FALSE,
            Self::Insensitive => TRUE,
        }
    }
}

/// Which of the two strings a [`CompareStringOrdinalError::TooLong`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operand {
    /// `lpString1`.
    First,
    /// `lpString2`.
    Second,
}

/// Why [`compare_string_ordinal`] gave no ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompareStringOrdinalError {
    /// A string is longer than the call's count can hold, so the call was not made.
    ///
    /// The page's syntax gives each count as an `int` (`cchCount1` and `cchCount2`: "Length of the string
    /// indicated by *lpString1*"), so a slice of more than `i32::MAX` units has no count to be passed
    /// with. This is the crate's own refusal and not the call's: the call was not made and Windows gave
    /// no code. When both strings are too long, the first is named.
    TooLong {
        /// The string that is too long.
        operand: Operand,
        /// Its length in `u16` units.
        len: usize,
    },
    /// The call failed and gave this code. The page lists `ERROR_INVALID_PARAMETER`, "Any of the
    /// parameter values was invalid"; a code of zero says the call gave a result the page does not list
    /// and no code.
    Call(Win32Error),
}

impl fmt::Display for CompareStringOrdinalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { operand, len } => write!(
                f,
                "the {} string of {len} units is longer than CompareStringOrdinal's count, an int, can hold",
                match operand {
                    Operand::First => "first",
                    Operand::Second => "second",
                }
            ),
            Self::Call(error) => write!(f, "CompareStringOrdinal failed: {error}"),
        }
    }
}

impl std::error::Error for CompareStringOrdinalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TooLong { .. } => None,
            Self::Call(error) => Some(error),
        }
    }
}

/// Compares two strings of UTF-16 code units by `CompareStringOrdinal`, which "tests for binary equality,
/// not linguistic equality".
///
/// Each slice is passed with its length, so it is compared as exactly the units it holds: a NUL is a unit,
/// an unpaired surrogate is a unit, and nothing past the end is read. The result is the first string's
/// place against the second's, which the page gives as `CSTR_LESS_THAN`, `CSTR_EQUAL` and
/// `CSTR_GREATER_THAN`.
///
/// # Errors
///
/// [`CompareStringOrdinalError::TooLong`] for a slice of more than `i32::MAX` units, without calling
/// `CompareStringOrdinal`. Otherwise the call's own failure, [`CompareStringOrdinalError::Call`].
pub fn compare_string_ordinal(
    first: &[u16],
    second: &[u16],
    case: Case,
) -> Result<Ordering, CompareStringOrdinalError> {
    let (first_count, second_count) = counts(first.len(), second.len())?;
    // SAFETY: each pointer is that of a slice and is valid for reads of exactly the count passed with it,
    // which is that slice's length, and neither count is -1.
    unsafe {
        call(
            first.as_ptr(),
            first_count,
            second.as_ptr(),
            second_count,
            case.ignore_case(),
        )
    }
    .map_err(CompareStringOrdinalError::Call)
}

/// The two lengths as the call's counts, or the first that does not fit.
fn counts(first: usize, second: usize) -> Result<(i32, i32), CompareStringOrdinalError> {
    let count = |operand, len| {
        i32::try_from(len).map_err(|_| CompareStringOrdinalError::TooLong { operand, len })
    };
    Ok((
        count(Operand::First, first)?,
        count(Operand::Second, second)?,
    ))
}

/// The call, and the code it left if it failed.
///
/// # Safety
///
/// `first` and `second` are valid for reads of `first_count` and `second_count` units, as the page
/// requires, or the counts are -1 and the strings are NUL-terminated.
unsafe fn call(
    first: *const u16,
    first_count: i32,
    second: *const u16,
    second_count: i32,
    ignore_case: i32,
) -> Result<Ordering, Win32Error> {
    // SAFETY: the caller's.
    let result =
        unsafe { CompareStringOrdinal(first, first_count, second, second_count, ignore_case) };
    // Nothing runs between the call and the read of its last error.
    ordering_of(result, Win32Error::last)
}

/// The ordering the call's result says, or the failure it is.
///
/// A zero is the failure the page gives, and `last_error` is read only then. A value the page does not
/// list is a failure with no code, which is how a code of zero is read: Windows gave none.
fn ordering_of(
    result: i32,
    last_error: impl FnOnce() -> Win32Error,
) -> Result<Ordering, Win32Error> {
    match result {
        CSTR_LESS_THAN => Ok(Ordering::Less),
        CSTR_EQUAL => Ok(Ordering::Equal),
        CSTR_GREATER_THAN => Ok(Ordering::Greater),
        0 => Err(last_error()),
        _ => Err(Win32Error::from_code(0)),
    }
}
