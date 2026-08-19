// Copyright (c) 2026 Mike Grier
//! The [`WtfEncoding`] storage seam and the [`Wtf16`] encoding.

/// A code-unit encoding for a [`WtfString`](crate::WtfString) / [`WtfStr`](crate::WtfStr).
///
/// This trait is the storage-width seam: the API common to every width is written
/// against `E: WtfEncoding`, while width-specific API (such as the `*const u16`
/// FFI surface) lives in inherent impls on the concrete instantiations. v1
/// implements only [`Wtf16`]; a `Wtf8` arm delegating to `OsString` slots into the
/// same seam later.
pub trait WtfEncoding {
    /// The code unit this encoding stores (`u16` for [`Wtf16`]).
    type Unit: Copy + Ord + core::hash::Hash + core::fmt::Debug;

    /// The NUL code unit (`U+0000`) used as the always-present buffer terminator.
    ///
    /// Changing this value is a breaking change to the storage format.
    const NUL: Self::Unit;

    /// Encode a UTF-8 `str` into this encoding's code units.
    fn encode_str(s: &str) -> Vec<Self::Unit>;

    /// Decode content code units to a `String` if they are well-formed for this
    /// encoding, or `None` if they are ill-formed (e.g. an unpaired surrogate),
    /// which a strict `String` cannot represent.
    fn decode(units: &[Self::Unit]) -> Option<String>;

    /// Decode content code units to a `String`, replacing any ill-formed sequence
    /// with the Unicode replacement character (`U+FFFD`).
    fn decode_lossy(units: &[Self::Unit]) -> String;
}

/// The WTF-16 encoding: arbitrary, ill-formed-surrogate-tolerant UTF-16 stored as
/// `u16` code units.
///
/// This is the v1 encoding and the representation Windows wide (`*W`) APIs consume
/// directly. It is a pure type-level marker and is never constructed.
pub enum Wtf16 {}

impl WtfEncoding for Wtf16 {
    type Unit = u16;
    const NUL: u16 = 0;

    fn encode_str(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn decode(units: &[u16]) -> Option<String> {
        String::from_utf16(units).ok()
    }

    fn decode_lossy(units: &[u16]) -> String {
        String::from_utf16_lossy(units)
    }
}
