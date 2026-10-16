// Copyright (c) Mike Grier.

//! The codes the tests in this crate share, so the set is defined once.
//!
//! They cover the places a conversion goes wrong: zero, the ordinary small codes, the 16-bit edge,
//! the 31-bit edge, and codes that need the top bit, which std holds as negative numbers and
//! which the earlier per-crate types lost or panicked on. Each is a `u32`, the `DWORD` carrier;
//! a test for a signed carrier casts.

/// `ERROR_SUCCESS`.
pub(crate) const SUCCESS: u32 = 0;
/// `ERROR_INVALID_FUNCTION`, the first nonzero code.
pub(crate) const INVALID_FUNCTION: u32 = 1;
/// `ERROR_FILE_NOT_FOUND`.
pub(crate) const FILE_NOT_FOUND: u32 = 2;
/// `ERROR_ACCESS_DENIED`.
pub(crate) const ACCESS_DENIED: u32 = 5;
/// `ERROR_INVALID_PARAMETER`.
pub(crate) const INVALID_PARAMETER: u32 = 87;
/// `ERROR_INSUFFICIENT_BUFFER`.
pub(crate) const INSUFFICIENT_BUFFER: u32 = 122;
/// `ERROR_IO_PENDING`.
pub(crate) const IO_PENDING: u32 = 997;
/// `ERROR_IO_DEVICE`.
pub(crate) const IO_DEVICE: u32 = 1117;
/// The largest value a 16-bit code holds.
pub(crate) const LARGEST_16_BIT: u32 = 0xFFFF;
/// The first value above 16 bits.
pub(crate) const FIRST_ABOVE_16_BIT: u32 = 0x1_0000;
/// The largest value an `i32` holds, so the last code std keeps as a non-negative number.
pub(crate) const LARGEST_NON_NEGATIVE: u32 = 0x7FFF_FFFF;
/// The first code std keeps as a negative number.
pub(crate) const FIRST_NEGATIVE: u32 = 0x8000_0000;
/// `ERROR_VIRTDISK_NOT_VIRTUAL_DISK`, a `WIN32_ERROR` above 2^31 that a real call returned.
pub(crate) const NOT_A_VIRTUAL_DISK: u32 = 0xC03A_0015;
/// Every bit set.
pub(crate) const ALL_BITS: u32 = u32::MAX;

pub(crate) const SAMPLES: [u32; 14] = [
    SUCCESS,
    INVALID_FUNCTION,
    FILE_NOT_FOUND,
    ACCESS_DENIED,
    INVALID_PARAMETER,
    INSUFFICIENT_BUFFER,
    IO_PENDING,
    IO_DEVICE,
    LARGEST_16_BIT,
    FIRST_ABOVE_16_BIT,
    LARGEST_NON_NEGATIVE,
    FIRST_NEGATIVE,
    NOT_A_VIRTUAL_DISK,
    ALL_BITS,
];
