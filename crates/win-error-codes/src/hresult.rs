// Copyright (c) Mike Grier.

use std::fmt;

use crate::Win32Error;

#[cfg(test)]
mod tests;

/// The severity bit: set for a failure, which is also the sign bit, so a failure is negative.
const SEVERITY_BIT: u32 = 0x8000_0000;
/// The customer bit: set for a code a third party defined and not Microsoft.
const CUSTOMER_BIT: u32 = 0x2000_0000;
/// `FACILITY_NT_BIT`: set on an `HRESULT` made from an `NTSTATUS` by `HRESULT_FROM_NT`.
const NTSTATUS_BIT: u32 = 0x1000_0000;
/// Where the facility starts.
const FACILITY_SHIFT: u32 = 16;
/// `HRESULT_FACILITY`'s mask, 13 bits: wider than the 11 bits the documentation draws, because the
/// macro takes the reserved bit and the `NTSTATUS` bit with it, and the DXGI facility, 0x87A, uses
/// the first of them.
const FACILITY_MASK: u32 = 0x1FFF;
/// The code field, which is the low 16 bits.
const CODE_MASK: u32 = 0x0000_FFFF;
/// The severity and everything above the code, which is what a wrapped Win32 code fixes.
const SEVERITY_AND_FACILITY: u32 = 0xFFFF_0000;
/// `FACILITY_WIN32`.
const FACILITY_WIN32: u32 = 7;
/// What the high half of an `HRESULT` that wraps a Win32 code is: severity set, `FACILITY_WIN32`,
/// and nothing else.
const WIN32_WRAP: u32 = SEVERITY_BIT | (FACILITY_WIN32 << FACILITY_SHIFT);

/// An `HRESULT`, the signed code COM and much of the Windows API report in.
///
/// An `i32`: `FAILED(hr)` is `hr < 0`, so the severity is the sign bit. The layout, from the top: the
/// severity, a reserved bit, the customer bit, the `NTSTATUS` bit, another reserved bit, the facility,
/// and a 16-bit code. It is a number space of its own, not [`Win32Error`]'s: a Win32 code is
/// wrapped into one by [`from_win32`](Self::from_win32) and read back out by
/// [`win32`](Self::win32), and neither direction is lossless in general (see each).
///
/// Success is any non-negative value, `S_OK` (zero) and `S_FALSE` (one) among them, which is why
/// [`into_result`](Self::into_result) can not tell those apart and the raw value is there for a
/// caller who must. It is not an `Error`, because a success is a value of this type.
///
/// # Example
///
/// ```
/// use win_error_codes::{HResult, Win32Error};
///
/// // ERROR_IO_DEVICE, wrapped as HRESULT_FROM_WIN32 wraps it.
/// let hresult = HResult::from_win32(Win32Error::from_code(1117));
/// assert_eq!(hresult.raw().cast_unsigned(), 0x8007_045D);
/// assert!(hresult.is_failure());
/// assert_eq!(hresult.win32(), Some(Win32Error::from_code(1117)));
/// assert_eq!(hresult.to_string(), "0x8007045D");
///
/// // E_FAIL is a failure that wraps no Win32 code.
/// let e_fail = HResult::from_raw(0x8000_4005_u32.cast_signed());
/// assert!(e_fail.is_failure());
/// assert_eq!(e_fail.win32(), None);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HResult(i32);

impl HResult {
    /// Wraps a raw `HRESULT`. Any value is accepted.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// The raw `HRESULT`, unaltered.
    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// `FAILED(hr)`: whether the severity bit is set, which is whether the value is negative.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        self.0 < 0
    }

    /// `SUCCEEDED(hr)`: any non-negative value, `S_OK` and `S_FALSE` among them.
    #[must_use]
    pub const fn is_success(self) -> bool {
        !self.is_failure()
    }

    /// `Ok` for a success and `Err` carrying this value for a failure.
    ///
    /// # Errors
    ///
    /// This value, when it is a failure.
    pub const fn into_result(self) -> Result<(), Self> {
        if self.is_success() { Ok(()) } else { Err(self) }
    }

    /// `HRESULT_CODE`: the low 16 bits.
    #[must_use]
    pub const fn code(self) -> u16 {
        (self.0.cast_unsigned() & CODE_MASK) as u16
    }

    /// `HRESULT_FACILITY`: the 13 bits above the code, as `winerror.h`'s macro takes them. That is
    /// wider than the 11-bit facility the documentation draws; see the note on the mask.
    #[must_use]
    pub const fn facility(self) -> u16 {
        ((self.0.cast_unsigned() >> FACILITY_SHIFT) & FACILITY_MASK) as u16
    }

    /// Whether the customer bit is set: a code a third party defined, not Microsoft.
    #[must_use]
    pub const fn is_customer(self) -> bool {
        self.0.cast_unsigned() & CUSTOMER_BIT != 0
    }

    /// Whether the `NTSTATUS` bit is set, which is what `HRESULT_FROM_NT` does to a status.
    #[must_use]
    pub const fn is_ntstatus_mapped(self) -> bool {
        self.0.cast_unsigned() & NTSTATUS_BIT != 0
    }

    /// `HRESULT_FROM_WIN32`: wraps a Win32 code with the severity bit and `FACILITY_WIN32`.
    ///
    /// **Not lossless, and that is the macro's own rule.** Zero is `S_OK`. A code that is not
    /// positive as an `i32`, so one of 2^31 or more, is taken to be an `HRESULT` already and is
    /// returned as it is. Any other code keeps only its low 16 bits: `0x0010045D` and `0x0000045D`
    /// both give `0x8007045D`. [`win32`](Self::win32) is the inverse only for the codes from 1 to
    /// 65535.
    #[must_use]
    pub const fn from_win32(error: Win32Error) -> Self {
        let code = error.code();
        if code.cast_signed() <= 0 {
            Self(code.cast_signed())
        } else {
            Self(((code & CODE_MASK) | WIN32_WRAP).cast_signed())
        }
    }

    /// The Win32 code this wraps, or `None` if it is not one `from_win32` makes: severity set,
    /// `FACILITY_WIN32`, and no other bit above the code.
    ///
    /// `None` for `S_OK`, though `from_win32` of zero is `S_OK`: a success wraps no error, which is
    /// the reading durable-ioring's `ErrorCode` already takes and tests. So the round trip holds for
    /// the codes from 1 to 65535 and not for zero or anything above.
    #[must_use]
    pub const fn win32(self) -> Option<Win32Error> {
        let bits = self.0.cast_unsigned();
        if bits & SEVERITY_AND_FACILITY == WIN32_WRAP {
            Some(Win32Error::from_code(bits & CODE_MASK))
        } else {
            None
        }
    }
}

/// The value in hex, as `winerror.h` writes it: `0x8007045D`.
impl fmt::Display for HResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010X}", self.0.cast_unsigned())
    }
}

/// `HResult(0x8007045D)`, in hex and not as the negative number the `i32` is.
impl fmt::Debug for HResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HResult({self})")
    }
}
