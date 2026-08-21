// Copyright (c) 2026 Mike Grier
//! The [`WtfEncoding`] storage seam and its [`Wtf16`] and [`Wtf8`] encodings.

use alloc::string::String;
use alloc::vec::Vec;

/// A code-unit encoding for a [`WtfString`](crate::WtfString) / [`WtfStr`](crate::WtfStr).
///
/// This trait is the storage-width seam: the API common to every width is written
/// against `E: WtfEncoding`, while width-specific API (such as the `*const u16`
/// FFI surface) lives in inherent impls on the concrete instantiations. The two
/// shipped encodings are [`Wtf16`] (`u16` units) and [`Wtf8`] (`u8` units); each
/// defines its own encode/decode/comparison/formatting semantics over a
/// crate-owned `Vec<Unit>` with the same always-terminated model.
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

/// The WTF-8 encoding: arbitrary, ill-formed-tolerant WTF-8 stored as `u8` code
/// units -- the byte representation a Windows `OsStr` uses.
///
/// It matches `OsString`'s WTF-8 layout but is not built on `OsString` (D-3): the
/// storage is a crate-owned `Vec<u8>`. Construction from units performs no
/// validation, so the bytes may be well-formed WTF-8 (including encoded
/// surrogates) or arbitrary. Like [`Wtf16`] it is a pure type-level marker and is
/// never constructed.
pub enum Wtf8 {}

impl WtfEncoding for Wtf8 {
    type Unit = u8;
    const NUL: u8 = 0;

    fn encode_str(s: &str) -> Vec<u8> {
        // A UTF-8 `str` is already valid WTF-8: encoding is the identity on bytes.
        s.as_bytes().to_vec()
    }

    fn decode(units: &[u8]) -> Option<String> {
        // Exact decode succeeds only for valid UTF-8; WTF-8-encoded surrogates and
        // arbitrary bytes are ill-formed for a strict `String`.
        core::str::from_utf8(units).ok().map(String::from)
    }

    fn decode_lossy(units: &[u8]) -> String {
        String::from_utf8_lossy(units).into_owned()
    }

    fn eq_str(units: &[u8], s: &str) -> bool {
        // A `str`'s bytes are its WTF-8 encoding; compare without allocating.
        units == s.as_bytes()
    }

    fn debug_fmt(units: &[u8], f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use core::fmt::Write as _;
        f.write_char('"')?;
        // Split into maximal valid-UTF-8 runs and ill-formed byte runs: valid runs
        // escape like a string, while each ill-formed byte is escaped losslessly
        // as `\xNN`, so distinct byte inputs stay distinguishable.
        for chunk in units.utf8_chunks() {
            for c in chunk.valid().chars() {
                // An apostrophe stays literal in string-style debug.
                if c == '\'' {
                    f.write_char('\'')?;
                } else {
                    for esc in c.escape_debug() {
                        f.write_char(esc)?;
                    }
                }
            }
            for &b in chunk.invalid() {
                write!(f, "\\x{b:02x}")?;
            }
        }
        f.write_char('"')
    }
}
