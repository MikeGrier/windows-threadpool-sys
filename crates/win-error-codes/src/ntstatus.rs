// Copyright (c) Mike Grier.

use std::fmt;

use crate::HResult;

#[cfg(test)]
mod tests;

/// Where the severity starts: the top two bits.
const SEVERITY_SHIFT: u32 = 30;
/// The severity is two bits.
const SEVERITY_MASK: u32 = 0b11;
/// The customer bit: set for a code a third party defined and not Microsoft.
const CUSTOMER_BIT: u32 = 0x2000_0000;
/// Where the facility starts.
const FACILITY_SHIFT: u32 = 16;
/// The facility is twelve bits, below the customer bit and the bit that `HRESULT_FROM_NT` sets.
const FACILITY_MASK: u32 = 0x0FFF;
/// The code field, which is the low 16 bits.
const CODE_MASK: u32 = 0x0000_FFFF;

/// How bad an `NTSTATUS` is, from its top two bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NtSeverity {
    /// `STATUS_SEVERITY_SUCCESS`: the operation succeeded.
    Success = 0,
    /// `STATUS_SEVERITY_INFORMATIONAL`: it succeeded, and has something to say.
    Informational = 1,
    /// `STATUS_SEVERITY_WARNING`: it did not wholly succeed. A failure to [`NtStatus::is_success`].
    Warning = 2,
    /// `STATUS_SEVERITY_ERROR`: it failed.
    Error = 3,
}

/// An `NTSTATUS`, the signed status the native API and the drivers beneath it report in.
///
/// An `i32`. The layout, from the top: a two-bit severity, the customer bit, a bit that
/// [`HResult::from_ntstatus`] sets when it makes an `HRESULT` of one, a twelve-bit facility, and a
/// 16-bit code. It is a number space of its own, not [`Win32Error`](crate::Win32Error)'s or
/// [`HResult`]'s, and **there is no conversion to a Win32 code here**: that is `RtlNtStatusToDosError`,
/// an `Rtl*` call, which this repository's rule on documented interfaces excludes. The one
/// conversion that is a macro, `HRESULT_FROM_NT`, is [`HResult::from_ntstatus`].
///
/// `NT_SUCCESS` is `status >= 0`, so a success and an informational status are both successes, and a
/// warning is not: [`is_success`](Self::is_success) and [`into_result`](Self::into_result) follow
/// that. It is not an `Error`, because a success is a value of this type.
///
/// # Example
///
/// ```
/// use win_error_codes::{HResult, NtSeverity, NtStatus};
///
/// // STATUS_ACCESS_VIOLATION.
/// let status = NtStatus::from_raw(0xC000_0005_u32.cast_signed());
/// assert_eq!(status.severity(), NtSeverity::Error);
/// assert!(!status.is_success());
/// assert_eq!(status.to_string(), "0xC0000005");
///
/// // STATUS_PENDING is a success.
/// assert!(NtStatus::from_raw(0x103).is_success());
///
/// // HRESULT_FROM_NT sets one bit, and the status is recovered by clearing it.
/// let hresult = HResult::from_ntstatus(status);
/// assert_eq!(hresult.raw().cast_unsigned(), 0xD000_0005);
/// assert_eq!(hresult.ntstatus(), Some(status));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NtStatus(i32);

impl NtStatus {
    /// Wraps a raw `NTSTATUS`. Any value is accepted.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// The raw `NTSTATUS`, unaltered.
    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// The severity, from the top two bits.
    #[must_use]
    pub const fn severity(self) -> NtSeverity {
        match (self.0.cast_unsigned() >> SEVERITY_SHIFT) & SEVERITY_MASK {
            0 => NtSeverity::Success,
            1 => NtSeverity::Informational,
            2 => NtSeverity::Warning,
            _ => NtSeverity::Error,
        }
    }

    /// `NT_SUCCESS`: whether the status is not negative, which is a success or an informational
    /// status. A warning is not.
    #[must_use]
    pub const fn is_success(self) -> bool {
        self.0 >= 0
    }

    /// `NT_INFORMATION`: whether the severity is informational.
    #[must_use]
    pub const fn is_information(self) -> bool {
        matches!(self.severity(), NtSeverity::Informational)
    }

    /// `NT_WARNING`: whether the severity is a warning.
    #[must_use]
    pub const fn is_warning(self) -> bool {
        matches!(self.severity(), NtSeverity::Warning)
    }

    /// `NT_ERROR`: whether the severity is an error.
    #[must_use]
    pub const fn is_error(self) -> bool {
        matches!(self.severity(), NtSeverity::Error)
    }

    /// `Ok` for `NT_SUCCESS` and `Err` carrying this status for anything else, a warning included.
    ///
    /// # Errors
    ///
    /// This status, when it is a warning or an error.
    pub const fn into_result(self) -> Result<(), Self> {
        if self.is_success() { Ok(()) } else { Err(self) }
    }

    /// The low 16 bits.
    #[must_use]
    pub const fn code(self) -> u16 {
        (self.0.cast_unsigned() & CODE_MASK) as u16
    }

    /// The twelve bits above the code.
    #[must_use]
    pub const fn facility(self) -> u16 {
        ((self.0.cast_unsigned() >> FACILITY_SHIFT) & FACILITY_MASK) as u16
    }

    /// Whether the customer bit is set: a code a third party defined, not Microsoft.
    #[must_use]
    pub const fn is_customer(self) -> bool {
        self.0.cast_unsigned() & CUSTOMER_BIT != 0
    }
}

impl HResult {
    /// `HRESULT_FROM_NT`: the status with the bit that marks an `HRESULT` made from one set.
    ///
    /// Not lossless for a status that already has that bit set: [`ntstatus`](Self::ntstatus) clears
    /// it, so such a status does not come back as it went in.
    #[must_use]
    pub const fn from_ntstatus(status: NtStatus) -> Self {
        Self::from_raw((status.raw().cast_unsigned() | crate::hresult::NTSTATUS_BIT).cast_signed())
    }

    /// The status this was made from, or `None` if the bit that `HRESULT_FROM_NT` sets is clear.
    #[must_use]
    pub const fn ntstatus(self) -> Option<NtStatus> {
        if self.is_ntstatus_mapped() {
            Some(NtStatus::from_raw(
                (self.raw().cast_unsigned() & !crate::hresult::NTSTATUS_BIT).cast_signed(),
            ))
        } else {
            None
        }
    }
}

/// The value in hex, as `ntstatus.h` writes it: `0xC0000005`.
impl fmt::Display for NtStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010X}", self.0.cast_unsigned())
    }
}

/// `NtStatus(0xC0000005)`, in hex and not as the negative number the `i32` is.
impl fmt::Debug for NtStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NtStatus({self})")
    }
}
