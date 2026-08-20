// Copyright (c) 2026 Mike Grier
//! The [`WtfEncoding`] storage seam and the [`Wtf16`] encoding.

/// A code-unit encoding for a [`WtfString`](crate::WtfString) / [`WtfStr`](crate::WtfStr).
///
/// This trait is the storage-width seam: the API common to every width is written
/// against `E: WtfEncoding`, while width-specific API (such as the `*const u16`
/// FFI surface) lives in inherent impls on the concrete instantiations. v1
/// implements only [`Wtf16`]; a `Wtf8` arm — `u8`/WTF-8 units with this crate's
/// own encode/decode/comparison/formatting semantics, backed by a crate-owned
/// `Vec<u8>` (the same always-terminated model as `Wtf16`, matching `OsString`'s
/// WTF-8 storage but not built on it) — slots into the same seam later.
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

    /// Whether content code `units` equal the UTF-8 `str` `s` under this encoding.
    ///
    /// The default encodes `s` and compares slices; an encoding can override with
    /// an allocation-free lazy comparison (as [`Wtf16`] does).
    fn eq_str(units: &[Self::Unit], s: &str) -> bool {
        units == Self::encode_str(s).as_slice()
    }

    /// Write the escaped debug form of `units`, like [`OsStr`](std::ffi::OsStr):
    /// quoted, with control and non-printable characters escaped.
    ///
    /// The default decodes lossily and escapes; an encoding can override to also
    /// escape *ill-formed* sequences losslessly (as [`Wtf16`] does for a lone
    /// surrogate), so distinct ill-formed inputs remain distinguishable.
    fn debug_fmt(units: &[Self::Unit], f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", Self::decode_lossy(units))
    }
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

    fn eq_str(units: &[u16], s: &str) -> bool {
        // Compare against the lazily-encoded UTF-16 of `s`; no allocation.
        units.iter().copied().eq(s.encode_utf16())
    }

    fn debug_fmt(units: &[u16], f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use core::fmt::Write as _;
        f.write_char('"')?;
        for unit in core::char::decode_utf16(units.iter().copied()) {
            match unit {
                // An apostrophe is literal inside a double-quoted (string-style)
                // debug; `char::escape_debug` would escape it like a char literal.
                Ok('\'') => f.write_char('\'')?,
                Ok(c) => {
                    for esc in c.escape_debug() {
                        f.write_char(esc)?;
                    }
                }
                // Preserve a lone surrogate losslessly as an escape, not U+FFFD.
                Err(e) => write!(f, "\\u{{{:x}}}", e.unpaired_surrogate())?,
            }
        }
        f.write_char('"')
    }
}
