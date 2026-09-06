// Copyright (c) 2026 Mike Grier

use std::format;
use std::string::String;
use std::vec::Vec;

use super::WtfEncoding;
use crate::WtfString;

enum DefaultEncoding {}

impl WtfEncoding for DefaultEncoding {
    type Unit = u8;
    const NUL: u8 = 0;

    fn encode_str(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    fn decode(units: &[u8]) -> Option<String> {
        core::str::from_utf8(units).ok().map(String::from)
    }

    fn decode_lossy(units: &[u8]) -> String {
        String::from_utf8_lossy(units).into_owned()
    }
}

#[test]
fn default_eq_str_compares_encoded_units() {
    for (units, text, expected) in [
        (&[][..], "", true),
        (b"a", "a", true),
        (b"abc", "abc", true),
        (b"a\0b", "a\0b", true),
        (b"line\n", "line\n", true),
        (b"", "a", false),
        (b"a", "", false),
        (b"abc", "abd", false),
        (b"abc", "ab", false),
        (b"ab", "abc", false),
        (b"A", "a", false),
        (b"a\0b", "ab", false),
    ] {
        assert_eq!(
            DefaultEncoding::eq_str(units, text),
            expected,
            "{units:?} vs {text:?}"
        );
    }
}

#[test]
fn default_debug_formats_lossy_decoding_like_a_string() {
    for (units, expected) in [
        (&[][..], "\"\""),
        (b"a", "\"a\""),
        (b"abc", "\"abc\""),
        (b"don't", "\"don't\""),
        (b"quote\"", "\"quote\\\"\""),
        (b"slash\\", "\"slash\\\\\""),
        (b"tab\t", "\"tab\\t\""),
        (b"line\n", "\"line\\n\""),
        (b"return\r", "\"return\\r\""),
        (b"nul\0", "\"nul\\0\""),
    ] {
        let value = WtfString::<DefaultEncoding>::from_units(units);
        assert_eq!(format!("{value:?}"), expected, "{units:?}");
    }
}
